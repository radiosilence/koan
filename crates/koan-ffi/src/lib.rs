//! uniffi bindings for kōan's engine: the native apps' view of

//! `koan-runtime`, which holds the facade itself. Every record, enum and

//! error is re-declared here for `uniffi::remote`, which checks each one

//! field for field against the runtime's; the objects wrap the runtime's

//! and delegate. Kept here, rather than derived in `koan-runtime`, because

//! uniffi names every FFI symbol after the crate that exports it, and the

//! Swift and Kotlin bindings stay those of `koan_ffi`. Each declaration

//! sits in a module named for the runtime file it mirrors, since uniffi

//! orders the bindings by module path.

#[cfg(target_os = "android")]
mod android;

use std::sync::Arc;

#[allow(unused_imports)]
use koan_runtime::{
    AccountRole, Album, AlbumOnDevice, AlbumPage, AlbumSort, ApiKeyInfo, AppPasswordInfo,
    Appearance, ArtEvictions, Artist, ArtistInfo, Assistants, AttestedKey, AutoEqEntry,
    AutoEqMaker, AutoEqOffer, BrowseChoices, BrowseFilter, CacheCleared, CommandNotice,
    ConnectedInfo, ConnectionInfo, CoverArt, CoverPreload, CreditPart, DeletedDsp, Device,
    DeviceInfo, DeviceReach, DspBand, DspBandBounds, DspBandKind, DspBrush, DspChannelLayout,
    DspCurve, DspDeviceAction, DspEarKind, DspEdited, DspEqJoin, DspImportPlan, DspImportRefusal,
    DspImportSummary, DspImpulse, DspInfo, DspJoin, DspLayerCurve, DspLayerInfo,
    DspMadeAgainstPreview, DspMeasurementReading, DspMixOutput, DspMixSource, DspOutputState,
    DspOverview, DspPoint, DspProfileDetail, DspProfileSummary, DspResponse, DspRole, DspSavedCopy,
    DspStarter, DspTargetName, DspTargetOption, DspTargets, DspTuningEntry, EntryStatus,
    FuzzyMatch, HeldInfo, ImportSummary, InfoField, Invite, InviteTo, KoanError, LibraryFolder,
    LyricLine, Lyrics, MixerEncoding, MixerFormat, MoveResult, NearbyServer, NewApiKey,
    NewAppPassword, NowPlaying, OrganizeEntry, OrganizePattern, OrganizePlan, OutputChoice,
    OutputInfo, OutputsInfo, PairingCode, PairingInfo, PairingLink, Placement, PlanOutcome,
    PlayHistoryEntry, PlayState, Playlist, PlaylistEntry, PlaylistExport, QrCode, QueueItem,
    QueueLock, RebuildSummary, RecentlyPlayed, ReleaseSection, RendererInfo, RendererOutput,
    RepeatMode, RunRole, ScanSummary, ScrobblingConnection, SearchKind, ServerAccount,
    ServerExtension, ServerQueue, ServerStanding, Settings, Share, ShareStatus, Sharing, ShelfKind,
    ShelfSummary, SleepState, SleepTimer, SquigHit, StateSlice, Stats, StreamFormat, SyncPhase,
    SyncProgress, SyncSummary, TailscaleStatus, TopShelf, TopShelfRecord, Track, TrackBrowseSort,
    TrackDetails, TrackListing, TrackSet, TrackSource, Transfer, TransferFigure, TransferState,
    TxtEntry, VizLevels,
};

uniffi::setup_scaffolding!();

/// What `fuzzy_search` matches against.
#[uniffi::remote(Enum)]
pub enum SearchKind {
    Track,
    Album,
    Artist,
}

#[uniffi::remote(Record)]
pub struct FuzzyMatch {
    pub id: i64,
    /// Pre-joined display text — the same string that was matched against.
    pub name: String,
    pub kind: SearchKind,
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

struct ProgressReporterBridge(Arc<dyn ProgressReporter>);

impl koan_runtime::ProgressReporter for ProgressReporterBridge {
    fn started(&self, total: u64) {
        self.0.started(total)
    }
    fn advanced(&self, done: u64, detail: String) {
        self.0.advanced(done, detail)
    }
}

/// App Attest, as the app does it for the push relay: `DCAppAttestService`.
/// Called from a background thread; each call blocks until the system has
/// answered.
#[uniffi::export(with_foreign)]
pub trait PushAttester: Send + Sync {
    /// Generate a key and attest it over `client_data_hash`; `None` where App
    /// Attest is unsupported or refuses.
    fn attest(&self, client_data_hash: Vec<u8>) -> Option<AttestedKey>;
    /// An assertion by the key `key_id` over `client_data_hash`.
    fn assert(&self, key_id: Vec<u8>, client_data_hash: Vec<u8>) -> Option<Vec<u8>>;
}

struct PushAttesterBridge(Arc<dyn PushAttester>);

impl koan_runtime::PushAttester for PushAttesterBridge {
    fn attest(&self, client_data_hash: Vec<u8>) -> Option<AttestedKey> {
        self.0.attest(client_data_hash)
    }
    fn assert(&self, key_id: Vec<u8>, client_data_hash: Vec<u8>) -> Option<Vec<u8>> {
        self.0.assert(key_id, client_data_hash)
    }
}

/// A key App Attest made and Apple attested.
#[uniffi::remote(Record)]
pub struct AttestedKey {
    pub key_id: Vec<u8>,
    pub attestation: Vec<u8>,
}

/// The iOS and tvOS audio session, which the app owns: activated by the
/// engine the moment before it plays, released by the player once output has
/// stopped for a while. Called on the player thread; `activate` must not
/// return until the session is active, and answers `None` if it could not be
/// made so. See `koan_core::audio::ios_backend::AudioSession`.
#[uniffi::export(with_foreign)]
pub trait AudioSessionHost: Send + Sync {
    /// Ask for `sample_rate` as the session's preferred rate, activate, and
    /// answer `AVAudioSession.sampleRate`; `None` if activation was refused.
    /// Also called on a session already active, when a track wants another
    /// rate.
    fn activate(&self, sample_rate: f64) -> Option<f64>;
    /// Ask for `sample_rate` without activating, and answer
    /// `AVAudioSession.sampleRate`: on a route change, when another app may
    /// hold the session.
    fn follow(&self, sample_rate: f64) -> f64;
    fn release(&self);
}

struct AudioSessionHostBridge(Arc<dyn AudioSessionHost>);

impl koan_runtime::AudioSessionHost for AudioSessionHostBridge {
    fn activate(&self, sample_rate: f64) -> Option<f64> {
        self.0.activate(sample_rate)
    }
    fn follow(&self, sample_rate: f64) -> f64 {
        self.0.follow(sample_rate)
    }
    fn release(&self) {
        self.0.release()
    }
}

/// A sample encoding a USB DAC's mixer can run in.
#[uniffi::remote(Enum)]
pub enum MixerEncoding {
    Pcm16,
    Pcm24Packed,
    Pcm32,
    Float,
}

/// One bit-perfect configuration the USB output offers: an
/// `AudioMixerAttributes` with `MIXER_BEHAVIOR_BIT_PERFECT`.
#[uniffi::remote(Record)]
pub struct MixerFormat {
    /// `AudioDeviceInfo.getId()`.
    pub device_id: i32,
    pub sample_rate: u32,
    pub channels: u16,
    pub encoding: MixerEncoding,
}

/// Android's `AudioManager`, for bit-perfect output to a USB DAC (Android
/// 14). Called on the player thread before each stream opens. See
/// `koan_core::audio::bit_perfect`.
#[uniffi::export(with_foreign)]
pub trait AndroidOutputHost: Send + Sync {
    /// `AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE`.
    fn mixer_rate(&self) -> u32;
    /// The USB output's bit-perfect formats; empty when the route is not USB,
    /// the system is older than Android 14, or bit-perfect output is off.
    fn offers(&self) -> Vec<MixerFormat>;
    /// `setPreferredMixerAttributes` for media with this format; `false` if
    /// Android refused.
    fn hold(&self, format: MixerFormat) -> bool;
    /// `clearPreferredMixerAttributes`.
    fn release(&self);
}

struct AndroidOutputHostBridge(Arc<dyn AndroidOutputHost>);

impl koan_runtime::AndroidOutputHost for AndroidOutputHostBridge {
    fn mixer_rate(&self) -> u32 {
        self.0.mixer_rate()
    }
    fn offers(&self) -> Vec<MixerFormat> {
        self.0.offers()
    }
    fn hold(&self, format: MixerFormat) -> bool {
        self.0.hold(format)
    }
    fn release(&self) {
        self.0.release()
    }
}

/// One entry of a DNS-SD TXT record.
#[uniffi::remote(Record)]
pub struct TxtEntry {
    pub key: String,
    pub value: String,
}

/// Android's `NsdManager`, for finding koan devices on the network and being
/// found: what Bonjour does on Apple platforms. See
/// `koan_core::remote::nearby::nsd`.
#[uniffi::export(with_foreign)]
pub trait NearbyHost: Send + Sync {
    /// Register this device as `name` on `port`, replacing any registration.
    fn advertise(&self, name: String, port: u16, txt: Vec<TxtEntry>);
    fn withdraw(&self);
    /// Discover and resolve `_koan._tcp` services, reporting each through
    /// `nearby_found` and `nearby_lost`, until `pause`.
    fn browse(&self);
    fn pause(&self);
}

struct NearbyHostBridge(Arc<dyn NearbyHost>);

impl koan_runtime::NearbyHost for NearbyHostBridge {
    fn advertise(&self, name: String, port: u16, txt: Vec<TxtEntry>) {
        self.0.advertise(name, port, txt)
    }
    fn withdraw(&self) {
        self.0.withdraw()
    }
    fn browse(&self) {
        self.0.browse()
    }
    fn pause(&self) {
        self.0.pause()
    }
}

/// Android's Wi-Fi, for finding UPnP renderers: the address to search from,
/// and the `MulticastLock` without which Android drops multicast. See
/// `koan_core::upnp::discovery::Network`.
#[uniffi::export(with_foreign)]
pub trait MulticastHost: Send + Sync {
    /// The Wi-Fi interface's IPv4 address, dotted.
    fn wifi_address(&self) -> Option<String>;
    fn hold_multicast(&self, on: bool);
}

struct MulticastHostBridge(Arc<dyn MulticastHost>);

impl koan_runtime::MulticastHost for MulticastHostBridge {
    fn wifi_address(&self) -> Option<String> {
        self.0.wifi_address()
    }
    fn hold_multicast(&self, on: bool) {
        self.0.hold_multicast(on)
    }
}

/// Where configuration, the library database and downloads live, for a
/// platform whose app directories only the app can name: Android's
/// `Context.filesDir`. Called once, before `KoanEngine::open`.
#[uniffi::export]
pub fn set_config_dir(path: String) {
    koan_runtime::set_config_dir(path)
}

/// The album sorts that mean something under `filter`, and with a search when
/// `searching`, in the order a menu offers them.
#[uniffi::export]
pub fn album_sorts_offered(filter: BrowseFilter, searching: bool) -> Vec<AlbumSort> {
    koan_runtime::album_sorts_offered(filter, searching)
}

/// As `album_sorts_offered`, for the track browser.
#[uniffi::export]
pub fn track_sorts_offered(filter: BrowseFilter, searching: bool) -> Vec<TrackBrowseSort> {
    koan_runtime::track_sorts_offered(filter, searching)
}

/// A size as koan reads one in the config ("50GB", "500 MB"), in bytes, or
/// `None` when it is not one. So the apps read the cache limit as the core
/// does, without a parser of their own.
#[uniffi::export]
pub fn size_in_bytes(text: String) -> Option<u64> {
    koan_runtime::size_in_bytes(text)
}

/// Whether the music of the device in view can be moved to `to` (this
/// device with `None`): `controlled` is the device controlled, `None` while
/// this one is in view. Both ends must play from the same library, and there
/// must be music to move: a queue here, or another device playing or paused
/// and in reach. Moving to the device already in view moves nothing.
#[uniffi::export]
pub fn can_move_music(
    to: Option<DeviceInfo>,
    controlled: Option<DeviceInfo>,
    has_queue: bool,
) -> bool {
    koan_runtime::can_move_music(to, controlled, has_queue)
}

/// A pairing code as it is being typed, as the device shows it: `XXXX-XXXX`
/// in capitals. Applied to the field on every change, so typing and pasting
/// read the same on every platform.
#[uniffi::export]
pub fn format_pair_code(typed: String) -> String {
    koan_runtime::format_pair_code(typed)
}

/// The ranges a band edited by hand is held to, and the band types it can
/// be: what an EQ editor clamps to and offers, so it never shows a value a
/// save would change.
#[uniffi::export]
pub fn dsp_band_bounds() -> DspBandBounds {
    koan_runtime::dsp_band_bounds()
}

/// The curve `points` pulled toward `db` at `hz` by `brush`: a step of
/// painting a graphic EQ on its graph.
#[uniffi::export]
pub fn dsp_paint_curve(points: Vec<DspPoint>, hz: f64, db: f64, brush: DspBrush) -> Vec<DspPoint> {
    koan_runtime::dsp_paint_curve(points, hz, db, brush)
}

/// A band's Q widened or narrowed by a vertical drag of `dy` points from
/// `start`, held to the range a band may have.
#[uniffi::export]
pub fn dsp_drag_q(start: f64, dy: f64) -> f64 {
    koan_runtime::dsp_drag_q(start, dy)
}

/// What an EQ's page offers on a device for the profile `name` of `role`,
/// where the device's correction is `active` and its filters hold the
/// profile where `in_chain`.
#[uniffi::export]
pub fn dsp_device_action(
    name: String,
    role: DspRole,
    active: Option<String>,
    in_chain: bool,
) -> DspDeviceAction {
    koan_runtime::dsp_device_action(name, role, active, in_chain)
}

/// The heading over a section of an artist's records.
#[uniffi::export]
pub fn release_section_title(section: ReleaseSection) -> String {
    koan_runtime::release_section_title(section)
}

/// `text` as a QR code, for a platform with no encoder of its own: Android.
/// Absent when the text is too long for one.
#[uniffi::export]
pub fn qr_code(text: String) -> Option<QrCode> {
    koan_runtime::qr_code(text)
}

/// The playhead at each whole second of playback, from the engine
/// (`KoanEngine::clock_stream`): what a time shown on screen moves on, with no
/// timer of the client's own.
#[derive(uniffi::Object)]
pub struct ClockStream(pub(crate) Arc<koan_runtime::ClockStream>);

#[uniffi::export]
impl ClockStream {
    /// The playhead, in milliseconds, at its next whole second while it
    /// plays, or as soon as it is anchored afresh (a seek, a pause, the next
    /// track). Waits through a pause, and past the end of the track until
    /// the next one's anchor. `None` once the engine is gone.
    pub async fn next(&self) -> Option<u64> {
        self.0.next().await
    }
}

/// How high a playing indicator's bars stand, from the levels a `VizStream`
/// delivers: koan-core's reading (`BarHeights`), each band against its own
/// recent loudest, so every app draws the same bars.
#[derive(uniffi::Object)]
pub struct BarHeights(pub(crate) Arc<koan_runtime::BarHeights>);

#[uniffi::export]
impl BarHeights {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(BarHeights(koan_runtime::BarHeights::new()))
    }

    /// A frame as it arrives: the bars' heights, 0 to 1, low band to high,
    /// or none when they did not move.
    pub fn take(&self, levels: VizLevels) -> Option<Vec<f32>> {
        self.0.take(levels)
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
pub struct VizStream(pub(crate) Arc<koan_runtime::VizStream>);

#[uniffi::export]
impl VizStream {
    /// The next frame, as three band energies. Waits until there is one.
    ///
    /// `None` once the engine is gone, which ends the caller's loop.
    pub async fn next(&self) -> Option<VizLevels> {
        self.0.next().await
    }
}

#[derive(uniffi::Object)]
pub struct KoanEngine(pub(crate) Arc<koan_runtime::KoanEngine>);

#[uniffi::export]
impl KoanEngine {
    /// Spawns the player thread and opens the library. One per process.
    /// `device_name` is what this device calls itself to the account's other
    /// devices; without one, the hostname. Named rather than primary: Kotlin
    /// has no async primary constructors.
    #[uniffi::constructor]
    pub async fn open(device_name: Option<String>) -> Result<Arc<Self>, KoanError> {
        koan_runtime::KoanEngine::open(device_name)
            .await
            .map(|x| Arc::new(KoanEngine(x)))
    }

    /// Move the cursor to `queue_item_id` and start playing it.
    pub async fn play(self: Arc<Self>, queue_item_id: String) -> Result<(), KoanError> {
        self.0.clone().play(queue_item_id).await
    }

    /// The app is in front again, perhaps after iOS suspended it: link now,
    /// prove every connection is alive and look at the network afresh, so
    /// the other devices are there the moment it opens. See
    /// `koan_core::remote::devices::resume`.
    pub fn link_nudge(&self) {
        self.0.link_nudge()
    }

    /// The device's network changed: Android's default network, or the
    /// Apple apps' path, came or went.
    /// Connections to the server start afresh and an unanswered server is
    /// tried now. See `koan_core::helpers::network_changed`.
    pub fn network_changed(&self) {
        self.0.network_changed()
    }

    /// The app went to the background, or came back. In the background
    /// nothing runs that nobody asked for — see `koan_core::quiet`.
    pub fn set_background(&self, background: bool) {
        self.0.set_background(background)
    }

    /// Whether the app is playing: a phone playing in the background stays
    /// findable on the network, one paused in the background does not.
    pub fn set_playing(&self, playing: bool) {
        self.0.set_playing(playing)
    }

    /// Stay awake in the background until `release_awake`: for a push, while
    /// the link takes what the server kept.
    pub fn hold_awake(&self) {
        self.0.hold_awake()
    }

    pub fn release_awake(&self) {
        self.0.release_awake()
    }

    /// Write to koan's log, beside the engine's own lines: for events only the
    /// app sees, such as an audio interruption, where the order relative to
    /// what the engine did is the whole point.
    pub fn log_note(&self, message: String) {
        self.0.log_note(message)
    }

    /// The accounts on this server that may control this device.
    pub fn device_shares(&self) -> Vec<String> {
        self.0.device_shares()
    }

    /// Let the account `grantee` control this device, or with `allow` false
    /// stop letting it: playback and the queue, nothing of this account. The
    /// list in `device_shares` follows once the server has it.
    pub fn share_device(&self, grantee: String, allow: bool) -> Result<(), KoanError> {
        self.0.share_device(grantee, allow)
    }

    /// Where Apple's push service reaches this app, as the OS issued it. It
    /// goes to kōan's push relay alone; servers get the relay's handles.
    /// `sandbox` for a development build.
    pub fn set_push_token(&self, token: String, sandbox: bool) {
        self.0.set_push_token(token, sandbox)
    }

    /// A notification the person tapped: `claim` is what it carried under
    /// `koan-e2e`, and `from` the slot the relay stamped on it (`koan-from`),
    /// which names the server. The link to that server sends it, and the
    /// server sends down the command it kept for it; links now if it is not
    /// up.
    pub fn claim_notification(&self, claim: String, from: Option<String>) {
        self.0.claim_notification(claim, from)
    }

    /// Let the app attest this install for the push relay: App Attest, which
    /// only it can reach. See `koan_core::remote::relay`.
    pub fn set_push_attester(&self, attester: Arc<dyn PushAttester>) {
        self.0
            .set_push_attester(Arc::new(PushAttesterBridge(attester)))
    }

    pub async fn pause(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().pause().await
    }

    pub async fn resume(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().resume().await
    }

    pub async fn stop(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().stop().await
    }

    /// Space-bar behaviour: pause when playing, resume otherwise.
    pub async fn toggle_play_pause(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().toggle_play_pause().await
    }

    pub async fn next(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().next().await
    }

    pub async fn previous(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().previous().await
    }

    /// Shuffle on or off, here or on the device being controlled. On, the
    /// tracks yet to play this pass play in a random order; the queue itself
    /// never moves.
    pub async fn set_shuffle(self: Arc<Self>, on: bool) -> Result<(), KoanError> {
        self.0.clone().set_shuffle(on).await
    }

    /// What follows a track at its end, here or on the device being
    /// controlled.
    pub async fn set_repeat(self: Arc<Self>, mode: RepeatMode) -> Result<(), KoanError> {
        self.0.clone().set_repeat(mode).await
    }

    /// Stop playback after a while or at the end of the track or record,
    /// fading out and pausing, here or on the device being controlled.
    pub async fn set_sleep_timer(self: Arc<Self>, timer: SleepTimer) -> Result<(), KoanError> {
        self.0.clone().set_sleep_timer(timer).await
    }

    pub async fn cancel_sleep_timer(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().cancel_sleep_timer().await
    }

    pub async fn seek(self: Arc<Self>, position_ms: u64) -> Result<(), KoanError> {
        self.0.clone().seek(position_ms).await
    }

    /// Follow the engine's state.
    ///
    /// The one way a client learns anything changed. Each answer is a batch of
    /// whole slices — see `state` for why they are snapshots and why the
    /// cursor cannot lose one. A fresh stream's first answer is the entire
    /// state, so there is nothing to seed from separately.
    pub fn observe(&self) -> Arc<StateStream> {
        Arc::new(StateStream(self.0.observe()))
    }

    /// Forget the transfers that have already settled. Running ones are left
    /// alone — stopping one is a different verb.
    pub fn clear_settled_downloads(&self) {
        self.0.clear_settled_downloads()
    }

    /// The byte counts of every transfer still going, read now.
    ///
    /// For a client drawing progress at its display's rate, which the `Figures`
    /// slice is not: it moves when a rate sample is taken, a few times a
    /// second. Synchronous because a display link has a frame to fill and no
    /// time to await one; it holds the download list's read lock for one pass
    /// over a few dozen entries and touches nothing else.
    pub fn transfer_readings(&self) -> Vec<TransferFigure> {
        self.0.transfer_readings()
    }

    /// Whether the player is playing, read from the engine now. For code that
    /// runs with the app in the background, where the mirror, refreshed for
    /// what is on screen, can still say what it said before.
    pub fn is_playing(&self) -> bool {
        self.0.is_playing()
    }

    /// Follow the spectrum, one message per analysed frame.
    ///
    /// The analyser is the clock. It runs at the rate the display asks for
    /// (see `set_viz_fps`), publishes a frame when it has one, and publishes
    /// nothing at all when the play head has stopped and the bars have fallen
    /// — so a paused koan delivers no messages, wakes nothing, and the thread
    /// that would have produced them is parked rather than looping.
    pub fn viz_stream(&self) -> Arc<VizStream> {
        Arc::new(VizStream(self.0.viz_stream()))
    }

    /// The playhead's whole seconds as it plays, for a client that shows the
    /// time: the engine wakes it at each, and while paused or with no
    /// subscriber nothing runs.
    pub fn clock_stream(&self) -> Arc<ClockStream> {
        Arc::new(ClockStream(self.0.clock_stream()))
    }

    /// Follow the spectrum as keyframes, for a client that draws the bars
    /// on the GPU and moves them between frames itself (the kōan GUI app):
    /// the analyser runs at the rate koan chooses for keyframes, which is far
    /// below a display's, and the client never names a rate.
    pub fn viz_keyframes(&self) -> Arc<VizStream> {
        Arc::new(VizStream(self.0.viz_keyframes()))
    }

    /// Run the analyser at the refresh rate of the display it is drawn on.
    ///
    /// koan cannot know that rate and a window can: 60 on one panel, 120 on
    /// another, and it changes when the window is dragged between them. One
    /// atomic store — the analyser picks it up on its next pass, and nothing
    /// wakes for it.
    pub fn set_viz_fps(&self, fps: u8) {
        self.0.set_viz_fps(fps)
    }

    /// Put the tracks `sources` name in the queue at `placement`. Returns the
    /// new queue item IDs.
    ///
    /// The sources are resolved here, on the command lane, so a command that
    /// names an artist takes its place in line the moment it is made rather
    /// than once its tracks have been read. Sources that come to nothing
    /// playable (a playlist with nothing downloaded while offline, a record a
    /// rescan removed) leave the queue as it was and fail with `NotFound`.
    pub async fn queue(
        self: Arc<Self>,
        sources: Vec<TrackSet>,
        placement: Placement,
    ) -> Result<Vec<String>, KoanError> {
        self.0.clone().queue(sources, placement).await
    }

    /// Append tracks. Starts playback if the player was stopped, and kicks off
    /// downloads for anything remote. Returns the new queue item IDs.
    pub async fn add_to_queue(
        self: Arc<Self>,
        track_ids: Vec<i64>,
    ) -> Result<Vec<String>, KoanError> {
        self.0.clone().add_to_queue(track_ids).await
    }

    /// Replace the queue, starting at `start_at` (default: the first track),
    /// and play it in order, or `shuffled` into a random order from its top,
    /// with shuffle play and repeat off: a play from a play button starts as
    /// asked whatever the modes were.
    ///
    /// The index is part of the command rather than a follow-up `play` because
    /// two commands means the first track audibly starts before the jump lands:
    /// clicking track nine of an album flashed track one as playing first.
    /// An index past the end starts at the beginning.
    pub async fn replace_queue(
        self: Arc<Self>,
        track_ids: Vec<i64>,
        start_at: Option<u32>,
        shuffled: bool,
    ) -> Result<Vec<String>, KoanError> {
        self.0
            .clone()
            .replace_queue(track_ids, start_at, shuffled)
            .await
    }

    /// Insert after an existing item — what a drop between two rows means.
    pub async fn insert_after(
        self: Arc<Self>,
        track_ids: Vec<i64>,
        after_queue_item_id: String,
    ) -> Result<Vec<String>, KoanError> {
        self.0
            .clone()
            .insert_after(track_ids, after_queue_item_id)
            .await
    }

    /// Removed as one undo step, however many IDs are passed.
    pub async fn remove_from_queue(
        self: Arc<Self>,
        queue_item_ids: Vec<String>,
    ) -> Result<(), KoanError> {
        self.0.clone().remove_from_queue(queue_item_ids).await
    }

    /// Reorder. `after` puts the items below the target rather than above.
    pub async fn move_in_queue(
        self: Arc<Self>,
        queue_item_ids: Vec<String>,
        target_queue_item_id: String,
        after: bool,
    ) -> Result<(), KoanError> {
        self.0
            .clone()
            .move_in_queue(queue_item_ids, target_queue_item_id, after)
            .await
    }

    /// Shuffle what follows the cursor, or the whole queue when there is
    /// none, as one undo step. Distinct from shuffle play mode, which
    /// leaves the queue's order alone.
    pub async fn shuffle_queue(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().shuffle_queue().await
    }

    pub async fn clear_queue(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().clear_queue().await
    }

    /// Take back the last edit to a playlist, or to the queue when none is
    /// named.
    pub async fn undo(self: Arc<Self>, playlist_id: Option<i64>) -> Result<(), KoanError> {
        self.0.clone().undo(playlist_id).await
    }

    /// Put back the last edit taken back, to a playlist or the queue.
    pub async fn redo(self: Arc<Self>, playlist_id: Option<i64>) -> Result<(), KoanError> {
        self.0.clone().redo(playlist_id).await
    }

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
        self.0.clone().artists(search, filter).await
    }

    pub async fn artist(self: Arc<Self>, artist_id: i64) -> Result<Option<Artist>, KoanError> {
        self.0.clone().artist(artist_id).await
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
        self.0
            .clone()
            .albums(artist_id, sort, seed, search, filter)
            .await
    }

    /// The records an artist features on and is not the album artist of, as
    /// their page orders their own: the "Features on" below them.
    pub async fn artist_features(self: Arc<Self>, artist_id: i64) -> Result<Vec<Album>, KoanError> {
        self.0.clone().artist_features(artist_id).await
    }

    /// What the browsers' codec and genre filters offer.
    pub async fn browse_choices(self: Arc<Self>) -> Result<BrowseChoices, KoanError> {
        self.0.clone().browse_choices().await
    }

    pub async fn album(self: Arc<Self>, album_id: i64) -> Result<Option<Album>, KoanError> {
        self.0.clone().album(album_id).await
    }

    /// Everything a record's page shows, in one call.
    ///
    /// One round trip and one pooled connection per click. As two parallel
    /// calls it would take two connections, and whenever the pool has to open
    /// one (schema DDL and a WAL checkpoint) a click costs about 300ms.
    pub async fn album_page(self: Arc<Self>, album_id: i64) -> Result<AlbumPage, KoanError> {
        self.0.clone().album_page(album_id).await
    }

    /// An album's tracks, or an artist's, in running order: by release, then
    /// disc and track. One of the two is required; the track browser is
    /// `track_listing`.
    pub async fn tracks(
        self: Arc<Self>,
        album_id: Option<i64>,
        artist_id: Option<i64>,
    ) -> Result<Vec<Track>, KoanError> {
        self.0.clone().tracks(album_id, artist_id).await
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
        self.0
            .clone()
            .track_listing(sort, search, filter, limit, offset)
            .await
    }

    pub async fn track(self: Arc<Self>, track_id: i64) -> Result<Option<Track>, KoanError> {
        self.0.clone().track(track_id).await
    }

    /// Everything known about a track, for its info view. `None` for a track
    /// that is not in the library.
    pub async fn track_details(
        self: Arc<Self>,
        track_id: i64,
    ) -> Result<Option<TrackDetails>, KoanError> {
        self.0.clone().track_details(track_id).await
    }

    /// FTS5 search across title, artist, album, genre.
    pub async fn search(
        self: Arc<Self>,
        query: String,
        limit: u32,
    ) -> Result<Vec<Track>, KoanError> {
        self.0.clone().search(query, limit).await
    }

    /// Fuzzy match — what the command palette wants. Ranked, best first, as
    /// every fuzzy search is (`queries::fuzzy_rank`).
    pub async fn fuzzy_search(
        self: Arc<Self>,
        query: String,
        kind: SearchKind,
        limit: u32,
    ) -> Result<Vec<FuzzyMatch>, KoanError> {
        self.0.clone().fuzzy_search(query, kind, limit).await
    }

    pub async fn random_tracks(
        self: Arc<Self>,
        count: u32,
        artist_id: Option<i64>,
    ) -> Result<Vec<Track>, KoanError> {
        self.0.clone().random_tracks(count, artist_id).await
    }

    pub async fn library_stats(self: Arc<Self>) -> Result<Stats, KoanError> {
        self.0.clone().library_stats().await
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
        self.0.clone().cover_art(track_id, size).await
    }

    /// Whether the device is on a metered or constrained network, or saving
    /// power. Fetching covers ahead waits while it is; playback and what a
    /// page asks for do not.
    pub fn set_network_constrained(&self, constrained: bool) {
        self.0.set_network_constrained(constrained)
    }

    /// Stop fetching covers ahead, keeping what was fetched, until the
    /// setting is turned on again or the app starts again.
    pub fn stop_cover_preload(&self) {
        self.0.stop_cover_preload()
    }

    /// How many cover requests to a server may run at once. The phone apps
    /// keep the engine's default; the desktop app bounds its own fetches by
    /// what is on screen and sets this to its ceiling.
    pub fn set_cover_fetch_limit(&self, limit: u32) {
        self.0.set_cover_fetch_limit(limit)
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
        self.0.clone().album_cover_art(album_id, size).await
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
        self.0.clone().remote_problem().await
    }

    /// Cached lyrics only — this never hits the network, so it is safe to call
    /// from a view body's task without stalling on LRCLIB.
    pub async fn lyrics(self: Arc<Self>, track_id: i64) -> Result<Option<Lyrics>, KoanError> {
        self.0.clone().lyrics(track_id).await
    }

    /// Cache, then LRCLIB. Hits the network on a miss; `lyrics()` answers from
    /// the cache alone and returns without one.
    pub async fn fetch_lyrics(self: Arc<Self>, track_id: i64) -> Result<Option<Lyrics>, KoanError> {
        self.0.clone().fetch_lyrics(track_id).await
    }

    /// LRCLIB asked by tags, for a queue entry the library no longer has: it
    /// plays from its own metadata, and nothing is cached for it.
    pub async fn lyrics_by_tags(
        self: Arc<Self>,
        artist: String,
        title: String,
        album: String,
        duration_ms: Option<u64>,
    ) -> Option<Lyrics> {
        self.0
            .clone()
            .lyrics_by_tags(artist, title, album, duration_ms)
            .await
    }

    /// Cached only — never the network, so a page can ask on every draw.
    pub async fn artist_info(
        self: Arc<Self>,
        artist_id: i64,
    ) -> Result<Option<ArtistInfo>, KoanError> {
        self.0.clone().artist_info(artist_id).await
    }

    /// The cache while fresh, otherwise MusicBrainz, Wikidata and Wikipedia.
    /// Seconds on a miss; an artist nothing can be found for is the normal
    /// case, not an error.
    pub async fn fetch_artist_info(
        self: Arc<Self>,
        artist_id: i64,
    ) -> Result<Option<ArtistInfo>, KoanError> {
        self.0.clone().fetch_artist_info(artist_id).await
    }

    /// The artist's photograph. Hits the network; the caller caches it.
    /// `None` only when the artist has no photograph: a download that failed
    /// is an error, worth asking for again.
    pub async fn artist_image(
        self: Arc<Self>,
        artist_id: i64,
    ) -> Result<Option<CoverArt>, KoanError> {
        self.0.clone().artist_image(artist_id).await
    }

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
        self.0.clone().play_history(search).await
    }

    /// What was played lately: the records, artists and tracks of the last
    /// `shelves::RECENT_DAYS`, at most `RECENT_LIMIT` of each, each once and
    /// newest first by its latest play, narrowed by `search`. The shelf, its
    /// window and its order are `koan_core::shelves`'.
    pub async fn recently_played(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<RecentlyPlayed, KoanError> {
        self.0.clone().recently_played(search).await
    }

    /// A shelf page: the first few artists, records and tracks on `shelf`,
    /// with how many there are of each. A section's heading opens the library's own
    /// listing with the same shelf as its filter; see `koan_core::shelves`.
    pub async fn shelf_summary(
        self: Arc<Self>,
        shelf: ShelfKind,
    ) -> Result<ShelfSummary, KoanError> {
        self.0.clone().shelf_summary(shelf).await
    }

    /// The same, `artists`, `albums` and `tracks` deep: what a search box
    /// that things are picked from lists.
    pub async fn shelf_summary_sized(
        self: Arc<Self>,
        shelf: ShelfKind,
        artists: u32,
        albums: u32,
        tracks: u32,
    ) -> Result<ShelfSummary, KoanError> {
        self.0
            .clone()
            .shelf_summary_sized(shelf, artists, albums, tracks)
            .await
    }

    /// The Apple TV's Top Shelf: the first `TOP_SHELF_LIMIT` records played
    /// lately and the latest favourited. Not narrowed to what this device
    /// holds while offline: the system shows the shelf when koan is not
    /// running, by which time the connection may be back.
    pub async fn top_shelf(self: Arc<Self>) -> Result<TopShelf, KoanError> {
        self.0.clone().top_shelf().await
    }

    /// The record holding `uid`, for a link that names one. Only a uid: a
    /// bare row id may by now be another record's.
    pub async fn album_for_uid(self: Arc<Self>, uid: String) -> Result<Option<i64>, KoanError> {
        self.0.clone().album_for_uid(uid).await
    }

    /// Turn offline mode on or off by hand: the library narrows to what can
    /// play here, as it does on its own when the server cannot be reached.
    pub fn set_offline(&self, on: bool) {
        self.0.set_offline(on)
    }

    /// Forget specific plays, here and, signed in to a koan server, on every
    /// device on the account. Returns how many entries were removed.
    pub async fn delete_plays(self: Arc<Self>, ids: Vec<i64>) -> Result<u32, KoanError> {
        self.0.clone().delete_plays(ids).await
    }

    /// Forget every play, here and, signed in to a koan server, on every
    /// device on the account. Returns how many entries were removed.
    pub async fn clear_play_history(self: Arc<Self>) -> Result<u32, KoanError> {
        self.0.clone().clear_play_history().await
    }

    /// Favourited tracks, narrowed by `search`.
    pub async fn favourites(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<Track>, KoanError> {
        self.0.clone().favourites(search).await
    }

    /// Favourited records, as rows, narrowed by `search`.
    pub async fn favourite_albums(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<Album>, KoanError> {
        self.0.clone().favourite_albums(search).await
    }

    /// Favourited artists, as rows, narrowed by `search`.
    pub async fn favourite_artists(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<Artist>, KoanError> {
        self.0.clone().favourite_artists(search).await
    }

    /// Returns the new state. Syncs to the remote server in the background when
    /// one is configured.
    pub async fn toggle_favourite(self: Arc<Self>, track_id: i64) -> Result<bool, KoanError> {
        self.0.clone().toggle_favourite(track_id).await
    }

    /// Toggle an album favourite. Returns the new state.
    pub async fn toggle_favourite_album(self: Arc<Self>, album_id: i64) -> Result<bool, KoanError> {
        self.0.clone().toggle_favourite_album(album_id).await
    }

    /// Toggle an artist favourite. Returns the new state.
    pub async fn toggle_favourite_artist(
        self: Arc<Self>,
        artist_id: i64,
    ) -> Result<bool, KoanError> {
        self.0.clone().toggle_favourite_artist(artist_id).await
    }

    /// Every favourited track id, for the UI to read row state from one place
    /// rather than from a copy baked into each row when it was fetched.
    pub async fn favourite_track_ids(self: Arc<Self>) -> Result<Vec<i64>, KoanError> {
        self.0.clone().favourite_track_ids().await
    }

    pub async fn favourite_album_ids(self: Arc<Self>) -> Result<Vec<i64>, KoanError> {
        self.0.clone().favourite_album_ids().await
    }

    pub async fn favourite_artist_ids(self: Arc<Self>) -> Result<Vec<i64>, KoanError> {
        self.0.clone().favourite_artist_ids().await
    }

    pub async fn playlists(self: Arc<Self>) -> Result<Vec<Playlist>, KoanError> {
        self.0.clone().playlists().await
    }

    /// The playlist's tracks, in playlist order. Duplicates are kept — the same
    /// song twice in a row is a thing people do on purpose.
    pub async fn playlist_tracks(
        self: Arc<Self>,
        playlist_id: i64,
    ) -> Result<Vec<PlaylistEntry>, KoanError> {
        self.0.clone().playlist_tracks(playlist_id).await
    }

    /// Up to four albums whose covers make the playlist's tile, in playlist
    /// order.
    pub async fn playlist_cover_album_ids(
        self: Arc<Self>,
        playlist_id: i64,
    ) -> Result<Vec<i64>, KoanError> {
        self.0.clone().playlist_cover_album_ids(playlist_id).await
    }

    pub async fn create_playlist(
        self: Arc<Self>,
        name: String,
        track_ids: Vec<i64>,
    ) -> Result<Playlist, KoanError> {
        self.0.clone().create_playlist(name, track_ids).await
    }

    pub async fn rename_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        name: String,
    ) -> Result<(), KoanError> {
        self.0.clone().rename_playlist(playlist_id, name).await
    }

    pub async fn delete_playlist(self: Arc<Self>, playlist_id: i64) -> Result<bool, KoanError> {
        self.0.clone().delete_playlist(playlist_id).await
    }

    /// Append the tracks `sources` name, resolved on the command lane. Returns
    /// how many landed.
    pub async fn add_to_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        sources: Vec<TrackSet>,
    ) -> Result<u32, KoanError> {
        self.0.clone().add_to_playlist(playlist_id, sources).await
    }

    /// Add the tracks `sources` name at a position rather than at the end —
    /// what dropping between two rows means.
    ///
    /// Both halves happen here, in one transaction, rather than as an add
    /// followed by a reorder from the caller: the caller would be reordering
    /// against the list as it was before its own insert landed.
    pub async fn insert_into_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        sources: Vec<TrackSet>,
        at: u32,
    ) -> Result<u32, KoanError> {
        self.0
            .clone()
            .insert_into_playlist(playlist_id, sources, at)
            .await
    }

    /// Put the entries in this order. Ids survive, so anything holding one —
    /// a queue item, say — still knows which row it means. Entries not named
    /// keep their order after the named ones.
    pub async fn reorder_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        entry_ids: Vec<i64>,
    ) -> Result<(), KoanError> {
        self.0
            .clone()
            .reorder_playlist(playlist_id, entry_ids)
            .await
    }

    /// Take entries out. Returns how many went.
    pub async fn remove_from_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        entry_ids: Vec<i64>,
    ) -> Result<u32, KoanError> {
        self.0
            .clone()
            .remove_from_playlist(playlist_id, entry_ids)
            .await
    }

    /// Shuffle the playlist itself, in place. Distinct from shuffling it into
    /// the queue, which leaves the playlist alone.
    pub async fn shuffle_playlist(self: Arc<Self>, playlist_id: i64) -> Result<(), KoanError> {
        self.0.clone().shuffle_playlist(playlist_id).await
    }

    /// How many shuffles undo would take back from the playlist before
    /// reaching any other edit. Moves only with an edit to the playlist, so
    /// read again whenever the playlist is. Async: an undo holds the history
    /// while it writes.
    pub async fn playlist_undo_shuffles(self: Arc<Self>, playlist_id: i64) -> u32 {
        self.0.clone().playlist_undo_shuffles(playlist_id).await
    }

    /// Move the entries named to just before the entry `before` (the end
    /// when `None`), among all of the playlist's entries, those a caller does
    /// not list included: offline, where only the entries with a file here
    /// are shown, a move among them leaves the rest where they are. The moved
    /// entries keep the order they had.
    pub async fn move_playlist_entries(
        self: Arc<Self>,
        playlist_id: i64,
        entry_ids: Vec<i64>,
        before: Option<i64>,
    ) -> Result<(), KoanError> {
        self.0
            .clone()
            .move_playlist_entries(playlist_id, entry_ids, before)
            .await
    }

    /// Add the tracks `sources` name just before the entry `before` (the end
    /// when `None`), among all of the playlist's entries, as
    /// `move_playlist_entries` places them. Returns how many landed.
    pub async fn insert_into_playlist_before(
        self: Arc<Self>,
        playlist_id: i64,
        sources: Vec<TrackSet>,
        before: Option<i64>,
    ) -> Result<u32, KoanError> {
        self.0
            .clone()
            .insert_into_playlist_before(playlist_id, sources, before)
            .await
    }

    /// Where the playlists sit in the sidebar, in the order given, sent on to
    /// a server that keeps the account's order (`koanPlaylistOrder`). On the
    /// command lane with the edits, so two drags land in the order they were
    /// made.
    pub async fn reorder_playlists(self: Arc<Self>, ids: Vec<i64>) -> Result<(), KoanError> {
        self.0.clone().reorder_playlists(ids).await
    }

    /// Remember whether this playlist is looked at grouped by album. `None`
    /// follows the app default, which for a playlist is ungrouped: a playlist
    /// is a sequence someone chose, not a shelf of records.
    pub async fn set_playlist_grouped(
        self: Arc<Self>,
        playlist_id: i64,
        grouped: Option<bool>,
    ) -> Result<(), KoanError> {
        self.0
            .clone()
            .set_playlist_grouped(playlist_id, grouped)
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
    /// The queue is the playlist's order, or with `shuffled` a random one,
    /// played from its top; shuffle play and repeat go off, as any play from
    /// a play button does.
    pub async fn play_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        start_entry: Option<i64>,
        shuffled: bool,
    ) -> Result<Vec<String>, KoanError> {
        self.0
            .clone()
            .play_playlist(playlist_id, start_entry, shuffled)
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
        self.0.clone().export_playlist(playlist_id, dest_path).await
    }

    /// Write the queue and position so the next launch can pick them up.
    /// Call it on quit; it is cheap enough to call on a timer too.
    ///
    /// The queue is written only when its contents changed since the last
    /// save. A cursor moving or a download landing changes the queue a client
    /// sees, not what is saved of it, so those save the position alone.
    pub async fn save_session(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().save_session().await
    }

    /// Persist where you are, without rewriting the queue.
    ///
    /// Cheap enough to call every second, which is what makes a crash cost a
    /// second of playback rather than the whole session. `save_session` still
    /// runs when the queue changes and on quit.
    pub async fn save_position(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().save_position().await
    }

    /// Whether this device keeps its queue in the account's play queue on the
    /// server, and what the server holds now, for asking before turning it on:
    /// turning it on replaces this device's queue with that one.
    pub async fn server_queue(self: Arc<Self>) -> Result<ServerQueue, KoanError> {
        self.0.clone().server_queue().await
    }

    /// Keep this device's queue in the account's play queue on the server, or
    /// stop. On, the server's queue replaces this device's, or this device's
    /// is saved there when the server has none; off leaves both as they are.
    pub async fn set_server_queue(self: Arc<Self>, on: bool) -> Result<(), KoanError> {
        self.0.clone().set_server_queue(on).await
    }

    /// Save the queue and playhead to the server now, if this device keeps
    /// them there: the app is going to the background or quitting.
    pub async fn save_server_queue_now(self: Arc<Self>) {
        self.0.clone().save_server_queue_now().await
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
        self.0.clone().restore_session().await
    }

    /// `restore_session`'s reading and sending, on the command lane.
    fn restore_blocking(&self) -> Result<u32, KoanError> {
        self.0.restore_blocking()
    }

    pub async fn devices(self: Arc<Self>) -> Result<Vec<Device>, KoanError> {
        self.0.clone().devices().await
    }

    pub async fn set_device(self: Arc<Self>, name: String) -> Result<(), KoanError> {
        self.0.clone().set_device(name).await
    }

    /// Albums, artists and tracks deleted since `after` (a `seq` this returned
    /// before; 0 the first time), for a cache keyed by their ids: SQLite reuses
    /// a freed id, and art cached under it would show for another record. Also
    /// those under a folder the library watcher rescanned, whose cover image
    /// beside the tracks may have changed. With `purge`, `after` is older than
    /// the entries kept or from another database, and the whole cache goes.
    pub async fn art_evictions(self: Arc<Self>, after: i64) -> Result<ArtEvictions, KoanError> {
        self.0.clone().art_evictions(after).await
    }

    /// Rebuild the audio output where playback is, keeping it paused if it
    /// was: after iOS stops the output for an interruption, the old unit will
    /// not start again.
    pub async fn restart_output(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().restart_output().await
    }

    pub async fn clear_device(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().clear_device().await
    }

    /// The device name persisted in config, or `None` for the system default.
    /// Read from config rather than the player so it survives a restart.
    pub async fn current_device(self: Arc<Self>) -> Option<String> {
        self.0.clone().current_device().await
    }

    /// Forget the device `id`, which is out of reach: out of the list until it
    /// is heard from again. One of this account's is forgotten by the server
    /// too, with its push token, and drops off the account's other devices.
    pub async fn forget_device(self: Arc<Self>, id: String) -> Result<(), KoanError> {
        self.0.clone().forget_device(id).await
    }

    /// Ask the account's Apple TV `id` to pair so that the account's devices
    /// can wake it and start koan there. Returns the name of the TV asked
    /// once it is showing a PIN; `finish_tv_pairing` takes it, within half a
    /// minute.
    pub async fn begin_tv_pairing(self: Arc<Self>, id: String) -> Result<String, KoanError> {
        self.0.clone().begin_tv_pairing(id).await
    }

    /// Finish the pairing under way with the PIN the TV shows. Returns once
    /// the server has kept it for every device of the account.
    pub async fn finish_tv_pairing(self: Arc<Self>, pin: String) -> Result<(), KoanError> {
        self.0.clone().finish_tv_pairing(pin).await
    }

    /// Abandon the pairing under way, which takes the PIN off the TV.
    pub fn cancel_tv_pairing(&self) {
        self.0.cancel_tv_pairing()
    }

    /// Stop waking the Apple TV `id` from any device of the account.
    pub async fn forget_tv_pairing(self: Arc<Self>, id: String) -> Result<(), KoanError> {
        self.0.clone().forget_tv_pairing(id).await
    }

    /// Hang up the connection to this device that `key` names
    /// (`ConnectedInfo::key`). To keep a stranger from dialling again, add its
    /// address to `devices_refused` too.
    pub fn end_connection(&self, key: u64) {
        self.0.end_connection(key)
    }

    /// Let a device disconnected with `end_connection` back in, by the
    /// address `HeldInfo` gives.
    pub fn allow_held(&self, addr: String) {
        self.0.allow_held(addr)
    }

    /// Control the device `id`, or this one with `None`. Picking a device is
    /// picking where music plays: this one pauses, and the transport, the
    /// queue and what is playing all show that device until another is
    /// picked. Nothing moves; see `move_music`.
    pub async fn control_device(self: Arc<Self>, id: Option<String>) -> Result<(), KoanError> {
        self.0.clone().control_device(id).await
    }

    /// Move the music of the device being controlled to `to` (this device
    /// with `None`): its queue and playhead go there, it pauses, and `to` is
    /// controlled from then on. `to` asleep is woken first, as choosing it
    /// does, while the music plays on where it is. Says how many tracks were
    /// left out because the server does not have them, which only this device
    /// can say, and whether `to` has said it has the music.
    pub async fn move_music(self: Arc<Self>, to: Option<String>) -> Result<MoveResult, KoanError> {
        self.0.clone().move_music(to).await
    }

    /// Send a link command, as JSON, to the device `id`. For a Live
    /// Activity's buttons, which run while iOS keeps the app's link down: it
    /// goes by the server first, which knows whether the device is linked.
    pub async fn command_device(
        self: Arc<Self>,
        id: String,
        command: String,
    ) -> Result<(), KoanError> {
        self.0.clone().command_device(id, command).await
    }

    /// Look for UPnP renderers on the network. They arrive in the
    /// `Renderers` slice over the next couple of seconds, and the list follows
    /// the network from then on.
    pub fn search_renderers(&self) {
        self.0.search_renderers()
    }

    /// The app is quitting: stop the renderer playing, if one is, waiting at
    /// most a second and a half for it. Blocks, on purpose: termination does
    /// not wait for a task. Call it after the last save, which records the
    /// session as playing, so the next launch carries on there.
    pub fn release_output(&self) {
        self.0.release_output()
    }

    /// Play the device in view through `output`: this one, or the device
    /// being controlled, which switches as its own menu would. The music
    /// carries on from where it is.
    ///
    /// Not on the ordered lane: opening a renderer session is a few round
    /// trips, and app commands queued behind it would wait them out.
    pub async fn select_output(self: Arc<Self>, output: OutputChoice) -> Result<(), KoanError> {
        self.0.clone().select_output(output).await
    }

    /// List this device's audio devices again, when the system says they
    /// changed or an output menu opens. The `Outputs` slice follows.
    pub async fn refresh_outputs(self: Arc<Self>) {
        self.0.clone().refresh_outputs().await
    }

    /// Ask the device being controlled to list its outputs again: its output
    /// menu was opened here. Over a route that is up now or not at all, since
    /// a device that is away has nothing new to say and is not worth waking.
    /// Its link state brings back whatever moved.
    pub async fn refresh_controlled_outputs(self: Arc<Self>) {
        self.0.clone().refresh_controlled_outputs().await
    }

    /// The volume of the renderer the device in view plays to, 0–100.
    pub async fn set_output_volume(self: Arc<Self>, volume: u8) -> Result<(), KoanError> {
        self.0.clone().set_output_volume(volume).await
    }

    /// Play the output `device` of the device in view through `profile`, or
    /// untouched with `None`. A renderer is named by its UDN.
    pub async fn set_output_preset(
        self: Arc<Self>,
        device: String,
        profile: Option<String>,
    ) -> Result<(), KoanError> {
        self.0.clone().set_output_preset(device, profile).await
    }

    /// Where the server should push updates to this app's Live Activity
    /// showing the device `device`; `None` once it has ended.
    pub fn set_live_activity(&self, token: Option<String>, device: Option<String>, sandbox: bool) {
        self.0.set_live_activity(token, device, sandbox)
    }

    pub async fn dsp_overview(self: Arc<Self>) -> DspOverview {
        self.0.clone().dsp_overview().await
    }

    /// The profiles, and what `device` plays: the output in use with `None`.
    /// A renderer is named by its UDN.
    pub async fn dsp_overview_for(self: Arc<Self>, device: Option<String>) -> DspOverview {
        self.0.clone().dsp_overview_for(device).await
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
        self.0.clone().dsp_import(paths, name, rate).await
    }

    /// What importing `paths` will do: a group of presets, or one profile
    /// combined from them all, with a name to suggest.
    pub async fn dsp_import_plan(self: Arc<Self>, paths: Vec<String>) -> DspImportPlan {
        self.0.clone().dsp_import_plan(paths).await
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
        self.0.clone().dsp_import_files(paths, name, rate).await
    }

    /// Play `member` of the group `group`, and none of the others.
    pub async fn dsp_select(
        self: Arc<Self>,
        group: String,
        member: String,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_select(group, member).await
    }

    /// Make `name` a group, one layer playing, or a stack of layers.
    pub async fn dsp_set_group(
        self: Arc<Self>,
        name: String,
        group: bool,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_set_group(name, group).await
    }

    /// Import text: shared from another app, or pasted.
    pub async fn dsp_import_text(
        self: Arc<Self>,
        text: String,
        name: Option<String>,
        rate: Option<u32>,
    ) -> Result<String, KoanError> {
        self.0.clone().dsp_import_text(text, name, rate).await
    }

    /// AutoEQ's results whose names match `query`, best first. The first
    /// search of the day may fetch the index.
    pub async fn autoeq_search(
        self: Arc<Self>,
        query: String,
        limit: u32,
    ) -> Result<Vec<AutoEqEntry>, KoanError> {
        self.0.clone().autoeq_search(query, limit).await
    }

    /// The makers in AutoEQ's index, alphabetically, with how many results
    /// each has.
    pub async fn autoeq_makers(self: Arc<Self>) -> Result<Vec<AutoEqMaker>, KoanError> {
        self.0.clone().autoeq_makers().await
    }

    /// `maker`'s results, by model, AutoEQ's preferred source first within
    /// each.
    pub async fn autoeq_models(
        self: Arc<Self>,
        maker: String,
    ) -> Result<Vec<AutoEqEntry>, KoanError> {
        self.0.clone().autoeq_models(maker).await
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
        self.0
            .clone()
            .autoeq_install(name, measured_by, device)
            .await
    }

    /// The AutoEQ result the output in use is, by its name, while it has no
    /// profile and the suggestion has not been turned down. `None` for a
    /// renderer, whose name is the user's to choose.
    pub async fn autoeq_suggestion(self: Arc<Self>) -> Option<AutoEqOffer> {
        self.0.clone().autoeq_suggestion().await
    }

    /// Stop suggesting an AutoEQ profile for the output in use.
    pub async fn autoeq_dismiss(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().autoeq_dismiss().await
    }

    /// The targets `name`'s correction can be moved to, for one installed
    /// from AutoEQ whose target is known.
    pub async fn dsp_targets(self: Arc<Self>, name: String) -> Option<DspTargets> {
        self.0.clone().dsp_targets(name).await
    }

    /// Say what `name` is for. A second correction in a chain is refused,
    /// naming the first.
    pub async fn dsp_set_role(
        self: Arc<Self>,
        name: String,
        role: crate::DspRole,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_set_role(name, role).await
    }

    /// The target a ready-made EQ was made for, by id, or `None` when it is
    /// not known, which leaves target switching off.
    pub async fn dsp_set_made_for(
        self: Arc<Self>,
        name: String,
        target: Option<String>,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_set_made_for(name, target).await
    }

    /// The targets for a kind of headphone, in-ear or over-ear, or for a
    /// speaker, with what each sounds like: shipped ones, then those added.
    pub async fn dsp_targets_for(self: Arc<Self>, ear: DspEarKind) -> Vec<DspTargetOption> {
        self.0.clone().dsp_targets_for(ear).await
    }

    /// What correcting the measurement in `text` to `target` would do, before
    /// it is saved: the measurement, the target, the predicted response and
    /// the EQ itself.
    pub async fn dsp_preview_measurement(
        self: Arc<Self>,
        text: String,
        target: String,
    ) -> Result<DspResponse, KoanError> {
        self.0.clone().dsp_preview_measurement(text, target).await
    }

    /// What `text` is read as, as a measurement — a speaker's or a
    /// headphone's, and for a speaker's Klippel export which of its curves —
    /// or why it cannot be read.
    pub async fn dsp_describe_measurement(
        self: Arc<Self>,
        text: String,
    ) -> Result<DspMeasurementReading, KoanError> {
        self.0.clone().dsp_describe_measurement(text).await
    }

    /// Measurements on squig.link sites whose name has each word of `query`,
    /// best first.
    pub async fn dsp_squig_search(
        self: Arc<Self>,
        query: String,
    ) -> Result<Vec<SquigHit>, KoanError> {
        self.0.clone().dsp_squig_search(query).await
    }

    /// The measurement `file` on the squig.link site `site`, its channels
    /// averaged, as frequency and level text for `dspSaveMeasured`.
    pub async fn dsp_squig_fetch(
        self: Arc<Self>,
        site: String,
        file: String,
    ) -> Result<String, KoanError> {
        self.0.clone().dsp_squig_fetch(site, file).await
    }

    /// Save the headphone or speaker `name`, measured as `text`, corrected
    /// to `target`, crediting `source` where the measurement came from one.
    pub async fn dsp_save_measured(
        self: Arc<Self>,
        name: String,
        text: String,
        ear: DspEarKind,
        target: String,
        source: Option<String>,
    ) -> Result<String, KoanError> {
        self.0
            .clone()
            .dsp_save_measured(name, text, ear, target, source)
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
        self.0.clone().dsp_preview_split(name, text, target).await
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
        self.0
            .clone()
            .dsp_split_baked(name, text, in_ear, target)
            .await
    }

    /// The account's EQs deleted in the last thirty days, newest first.
    pub async fn dsp_deleted(self: Arc<Self>) -> Result<Vec<DeletedDsp>, KoanError> {
        self.0.clone().dsp_deleted().await
    }

    /// Bring a deleted EQ back on every device, from the server's copy.
    pub async fn dsp_restore(self: Arc<Self>, uid: String) -> Result<(), KoanError> {
        self.0.clone().dsp_restore(uid).await
    }

    /// Move `name`'s correction to the target `id`, or with `None` back to the
    /// one it was made for.
    pub async fn dsp_choose_target(
        self: Arc<Self>,
        name: String,
        id: Option<String>,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_choose_target(name, id).await
    }

    /// Add a target from a CSV of frequency and level, or a squig.link
    /// export, to choose from for every correction of its kind.
    pub async fn dsp_add_target(
        self: Arc<Self>,
        path: String,
    ) -> Result<DspTargetOption, KoanError> {
        self.0.clone().dsp_add_target(path).await
    }

    /// Make `name` a stack of `layers`, in order, creating it if there is
    /// none. Refused where it could not play.
    pub async fn dsp_set_layers(
        self: Arc<Self>,
        name: String,
        layers: Vec<DspLayerInfo>,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_set_layers(name, layers).await
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
        self.0
            .clone()
            .dsp_set_band(name, index, kind, freq, gain_db, q)
            .await
    }

    /// `dsp_set_band`, unless filter `index` is no longer `expected`, the
    /// band as the caller last read it: an index names a place in the list,
    /// and a filter added or removed elsewhere would put another there. Says
    /// what it stored, which an edit following this one names.
    #[allow(clippy::too_many_arguments)]
    pub async fn dsp_edit_band(
        self: Arc<Self>,
        name: String,
        index: u32,
        expected: DspBand,
        kind: String,
        freq: f64,
        gain_db: f64,
        q: f64,
    ) -> Result<DspEdited, KoanError> {
        self.0
            .clone()
            .dsp_edit_band(name, index, expected, kind, freq, gain_db, q)
            .await
    }

    /// `dsp_set_curve`, unless filter `index` is no longer `expected`, as
    /// `dsp_edit_band` is.
    pub async fn dsp_edit_curve(
        self: Arc<Self>,
        name: String,
        index: u32,
        expected: DspBand,
        points: Vec<DspPoint>,
    ) -> Result<DspEdited, KoanError> {
        self.0
            .clone()
            .dsp_edit_curve(name, index, expected, points)
            .await
    }

    /// Set the points of `name`'s graphic curve `index`, held within a
    /// band's ranges and put in order of frequency.
    pub async fn dsp_set_curve(
        self: Arc<Self>,
        name: String,
        index: u32,
        points: Vec<DspPoint>,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_set_curve(name, index, points).await
    }

    /// Add a flat band at 1 kHz to `name`, on `channel` alone or on both;
    /// its index among the filters.
    pub async fn dsp_add_band(
        self: Arc<Self>,
        name: String,
        channel: Option<u16>,
    ) -> Result<u32, KoanError> {
        self.0.clone().dsp_add_band(name, channel).await
    }

    /// Make the EQ `name`, eight bands from `starter`.
    pub async fn dsp_create_eq(
        self: Arc<Self>,
        name: String,
        starter: DspStarter,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_create_eq(name, starter).await
    }

    /// Give `name` a list of bands for each channel, or one for both from
    /// channel `keep`.
    pub async fn dsp_set_stereo(
        self: Arc<Self>,
        name: String,
        stereo: bool,
        keep: u16,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_set_stereo(name, stereo, keep).await
    }

    /// Have `device` play filters alone, its correction taken off, or
    /// correct for a headphone again.
    pub async fn dsp_set_filters_only(
        self: Arc<Self>,
        device: String,
        only: bool,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_set_filters_only(device, only).await
    }

    /// Take filter `index` out of `name`.
    pub async fn dsp_remove_filter(
        self: Arc<Self>,
        name: String,
        index: u32,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_remove_filter(name, index).await
    }

    /// `dsp_remove_filter`, unless filter `index` is no longer `expected`,
    /// the filter as the caller last read it, as `dsp_edit_band` is.
    pub async fn dsp_remove_edited(
        self: Arc<Self>,
        name: String,
        index: u32,
        expected: DspBand,
    ) -> Result<DspEdited, KoanError> {
        self.0
            .clone()
            .dsp_remove_edited(name, index, expected)
            .await
    }

    /// What the output in use plays: its correction, and the tuning on top
    /// adjusted to the correction's target.
    pub async fn dsp_output_response(self: Arc<Self>, rate: u32) -> Option<DspResponse> {
        self.0.clone().dsp_output_response(rate).await
    }

    /// What `device` plays, whether or not it is the output in use.
    pub async fn dsp_output_response_for(
        self: Arc<Self>,
        device: String,
        rate: u32,
    ) -> Option<DspResponse> {
        self.0.clone().dsp_output_response_for(device, rate).await
    }

    /// Make `device`'s tuning `tuning`: EQs in the order they play, each on
    /// or off.
    pub async fn dsp_set_tunings(
        self: Arc<Self>,
        device: String,
        tuning: Vec<DspTuningEntry>,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_set_tunings(device, tuning).await
    }

    /// Save `device`'s correction and tuning as the preset `name`.
    pub async fn dsp_save_preset(
        self: Arc<Self>,
        device: String,
        name: String,
    ) -> Result<String, KoanError> {
        self.0.clone().dsp_save_preset(device, name).await
    }

    /// Set `device` from the preset `name`, or flat with `None`.
    pub async fn dsp_apply_preset(
        self: Arc<Self>,
        device: String,
        name: Option<String>,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_apply_preset(device, name).await
    }

    /// Put `name` back as it was imported.
    pub async fn dsp_revert(self: Arc<Self>, name: String) -> Result<(), KoanError> {
        self.0.clone().dsp_revert(name).await
    }

    /// A copy of `name` as it is now, under `new` or "<name> copy". The
    /// copy's name.
    pub async fn dsp_duplicate(
        self: Arc<Self>,
        name: String,
        new: Option<String>,
    ) -> Result<String, KoanError> {
        self.0.clone().dsp_duplicate(name, new).await
    }

    /// `name`'s filters and headroom as they are, for `dsp_save_as_copy` to
    /// put back. Opaque.
    pub async fn dsp_snapshot(self: Arc<Self>, name: String) -> Option<String> {
        self.0.clone().dsp_snapshot(name).await
    }

    /// Keep `name` as it is now as a copy, under `new` or "<name> copy", and
    /// put `name` back to `before` (a `dsp_snapshot`), or to its file. On
    /// `device`, the copy takes its place in the tuning where the tuning names
    /// it.
    pub async fn dsp_save_as_copy(
        self: Arc<Self>,
        name: String,
        new: Option<String>,
        before: Option<String>,
        device: Option<String>,
    ) -> Result<DspSavedCopy, KoanError> {
        self.0
            .clone()
            .dsp_save_as_copy(name, new, before, device)
            .await
    }

    /// The target the tuning `name` was made against, or `None` when that is
    /// not known, which plays it as it is on any correction.
    pub async fn dsp_set_tuned_for(
        self: Arc<Self>,
        name: String,
        target: Option<String>,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_set_tuned_for(name, target).await
    }

    /// What the tuning `name` looks made against on `device` (the output in
    /// use with `None`), where it does not say.
    pub async fn dsp_suggest_made_against(
        self: Arc<Self>,
        name: String,
        device: Option<String>,
    ) -> Option<DspTargetName> {
        self.0.clone().dsp_suggest_made_against(name, device).await
    }

    /// What the tuning `name` adds on `device`'s correction for each target
    /// it could be said to be made against.
    pub async fn dsp_made_against_previews(
        self: Arc<Self>,
        name: String,
        device: Option<String>,
    ) -> Vec<DspMadeAgainstPreview> {
        self.0.clone().dsp_made_against_previews(name, device).await
    }

    /// What `name` does to the sound at `rate`, for drawing. `None` for a
    /// profile that is not there or would not play.
    pub async fn dsp_response(self: Arc<Self>, name: String, rate: u32) -> Option<DspResponse> {
        self.0.clone().dsp_response(name, rate).await
    }

    /// What `name` would draw with band `index` set so, nothing saved: for
    /// the graph to follow a drag.
    #[allow(clippy::too_many_arguments)]
    pub async fn dsp_preview_band(
        self: Arc<Self>,
        name: String,
        index: u32,
        kind: String,
        freq: f64,
        gain_db: f64,
        q: f64,
        rate: u32,
    ) -> Option<DspResponse> {
        self.0
            .clone()
            .dsp_preview_band(name, index, kind, freq, gain_db, q, rate)
            .await
    }

    /// What `name` would draw with curve `index` set to `points`, nothing
    /// saved.
    pub async fn dsp_preview_curve(
        self: Arc<Self>,
        name: String,
        index: u32,
        points: Vec<DspPoint>,
        rate: u32,
    ) -> Option<DspResponse> {
        self.0
            .clone()
            .dsp_preview_curve(name, index, points, rate)
            .await
    }

    pub async fn dsp_detail(self: Arc<Self>, name: String) -> Option<DspProfileDetail> {
        self.0.clone().dsp_detail(name).await
    }

    /// Keep `name` on every device of the account, or on this one alone.
    pub async fn dsp_set_scope(
        self: Arc<Self>,
        name: String,
        everywhere: bool,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_set_scope(name, everywhere).await
    }

    pub async fn dsp_rename(self: Arc<Self>, old: String, new: String) -> Result<(), KoanError> {
        self.0.clone().dsp_rename(old, new).await
    }

    /// Play the current output through `profile`, or untouched with `None`.
    pub async fn dsp_assign(self: Arc<Self>, profile: Option<String>) -> Result<(), KoanError> {
        self.0.clone().dsp_assign(profile).await
    }

    /// Play `device` through `profile`, or untouched with `None`, whether or
    /// not it is the output in use. A renderer is named by its UDN.
    pub async fn dsp_assign_device(
        self: Arc<Self>,
        device: String,
        profile: Option<String>,
    ) -> Result<(), KoanError> {
        self.0.clone().dsp_assign_device(device, profile).await
    }

    /// Delete the profile `name` and its files. Taking an EQ out of a chain
    /// or a stack is `dsp_set_tunings` or `dsp_set_layers`, never this.
    pub async fn dsp_delete(self: Arc<Self>, name: String) -> Result<(), KoanError> {
        self.0.clone().dsp_delete(name).await
    }

    /// Hand the engine the app's audio session, before anything can play.
    /// Does nothing off iOS and tvOS.
    pub fn set_audio_session(&self, host: Arc<dyn AudioSessionHost>) {
        self.0
            .set_audio_session(Arc::new(AudioSessionHostBridge(host)))
    }

    /// Hand the engine Android's `NsdManager`, to find koan devices on the
    /// network and be found. Does nothing off Android.
    pub fn set_nearby_host(&self, host: Arc<dyn NearbyHost>) {
        self.0.set_nearby_host(Arc::new(NearbyHostBridge(host)))
    }

    /// A koan device the app's `NsdManager` resolved. Does nothing off
    /// Android.
    pub fn nearby_found(&self, name: String, host: String, port: u16, txt: Vec<TxtEntry>) {
        self.0.nearby_found(name, host, port, txt)
    }

    /// A koan device's service gone from the network.
    pub fn nearby_lost(&self, name: String) {
        self.0.nearby_lost(name)
    }

    /// Whether the system refuses the app the local network: Android 17's
    /// permission, denied. Shown where Settings explains it on Apple.
    pub fn nearby_blocked(&self, blocked: bool) {
        self.0.nearby_blocked(blocked)
    }

    /// Hand the engine the platform's Wi-Fi, for UPnP discovery: the address
    /// searches go out of, and the lock that lets multicast in while they run.
    pub fn set_multicast_host(&self, host: Arc<dyn MulticastHost>) {
        self.0
            .set_multicast_host(Arc::new(MulticastHostBridge(host)))
    }

    /// Hand the engine Android's `AudioManager`, for bit-perfect output to a
    /// USB DAC. Until it is given, output goes through the system mixer.
    pub fn set_output_host(&self, host: Arc<dyn AndroidOutputHost>) {
        self.0
            .set_output_host(Arc::new(AndroidOutputHostBridge(host)))
    }

    /// The name of the port iOS routes audio to, on each route change: what
    /// profiles are chosen by on a phone, and the cue to read the session's
    /// rate again. On Android, the media route's name, which is what the
    /// phone's output is called; its output is restarted by the app. Does
    /// nothing elsewhere.
    pub async fn set_audio_route(self: Arc<Self>, name: String) -> Result<(), KoanError> {
        self.0.clone().set_audio_route(name).await
    }

    /// The kōan theme, or the platform's look. Saved in the background; it
    /// takes effect on the next launch, since every view is drawn in the theme
    /// read at start.
    pub fn set_theme(&self, koan: bool) {
        self.0.set_theme(koan)
    }

    /// Colours from the record, or koan's own. Saved in the background; the
    /// app redraws from its own copy.
    pub fn set_record_colours(&self, on: bool) {
        self.0.set_record_colours(on)
    }

    /// The wash under the whole window, or panels on grounds of their own.
    /// Saved in the background; the app redraws from its own copy.
    pub fn set_wash_window(&self, on: bool) {
        self.0.set_wash_window(on)
    }

    /// The rainbow, on this device. Saved in the background; the app redraws
    /// from its own copy.
    pub fn set_rainbow(&self, on: bool) {
        self.0.set_rainbow(on)
    }

    /// Show icons beside labels in the kōan theme, or not. Saved in the
    /// background; the app redraws from its own copy.
    pub fn set_theme_icons(&self, on: bool) {
        self.0.set_theme_icons(on)
    }

    /// Text size on the Mac, in percent (see `appearance.text_size`). Saved in
    /// the background; the app redraws from its own copy.
    pub fn set_text_size(&self, percent: u16) {
        self.0.set_text_size(percent)
    }

    /// How far the kōan theme holds the wash to its contrast limit, in percent
    /// (see `appearance.wash_tint`). Saved in the background; the app redraws
    /// from its own copy.
    pub fn set_wash_tint(&self, percent: u8) {
        self.0.set_wash_tint(percent)
    }

    /// How fast the kōan theme's wash drifts, in percent of its own pace (see
    /// `appearance.wash_drift`). Saved in the background; the app redraws
    /// from its own copy.
    pub fn set_wash_drift(&self, percent: u16) {
        self.0.set_wash_drift(percent)
    }

    /// How far the kōan theme's wash turns as it drifts, in degrees (see
    /// `appearance.wash_turn`). Saved in the background; the app redraws from
    /// its own copy.
    pub fn set_wash_turn(&self, degrees: u8) {
        self.0.set_wash_turn(degrees)
    }

    /// Wait for every setting still being saved to land. For the app to call as
    /// it quits, since a change made just before would otherwise go with the
    /// process: the theme, which takes effect on the next launch, most of all.
    /// Gives up after two seconds, so a disk that has gone away cannot hang the
    /// quit.
    pub fn flush_settings(&self) {
        self.0.flush_settings()
    }

    /// How the app is drawn, from `[appearance]`, once every change still
    /// being saved has landed. Read once, as the app opens: a change takes
    /// effect on the next launch.
    pub fn appearance(&self) -> Appearance {
        self.0.appearance()
    }

    /// The whole configuration, as the settings window shows it.
    pub async fn settings(self: Arc<Self>) -> Settings {
        self.0.clone().settings().await
    }

    /// Write the settings back.
    ///
    /// Each setting lands in the file that owns it — `Config::persist` routes
    /// folders and the server account to `config.local.toml` and taste like
    /// ReplayGain to `config.toml`. The password is not here; it goes through
    /// `sign_in_remote`.
    pub async fn update_settings(self: Arc<Self>, s: Settings) -> Result<(), KoanError> {
        self.0.clone().update_settings(s).await
    }

    /// Write back what differs between `read`, the settings as the caller
    /// read them, and `s`, the same with its edits made: a field nobody here
    /// edited keeps whatever the file holds now, though a sign-in, another
    /// window or the terminal UI wrote it after `read` was taken.
    pub async fn update_settings_since(
        self: Arc<Self>,
        read: Settings,
        s: Settings,
    ) -> Result<(), KoanError> {
        self.0.clone().update_settings_since(read, s).await
    }

    /// Ask the running library task to stop.
    ///
    /// It stops between transactions and keeps what it had already committed —
    /// a cancelled scan is a shorter scan, not an undone one.
    pub fn cancel_library_task(&self) {
        self.0.cancel_library_task()
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
        self.0.clone().sign_in_remote(url, username, password).await
    }

    /// Sign in to a koan server with an API key the account already holds.
    pub async fn sign_in_remote_with_key(
        self: Arc<Self>,
        url: String,
        username: String,
        api_key: String,
    ) -> Result<(), KoanError> {
        self.0
            .clone()
            .sign_in_remote_with_key(url, username, api_key)
            .await
    }

    /// Join a server with an invite: its token traded for an API key of this
    /// device's own, or the password a pasted address carries. Checked against
    /// the server before anything is written.
    pub async fn join_invite(self: Arc<Self>, invite: Invite) -> Result<(), KoanError> {
        self.0.clone().join_invite(invite).await
    }

    /// Read an invite: the koan.rocks link, `koan://join`, or a server address
    /// with the account in it. `None` for anything else, so a field can offer
    /// to join only when what was pasted is one.
    pub fn parse_invite(&self, link: String) -> Option<Invite> {
        self.0.parse_invite(link)
    }

    /// Ask the server at `url` to sign this device in, once someone signed in
    /// elsewhere approves it. The code and link come back to show;
    /// `await_pairing` waits for the answer. A pairing already waiting is
    /// given up.
    pub async fn start_pairing(self: Arc<Self>, url: String) -> Result<PairingCode, KoanError> {
        self.0.clone().start_pairing(url).await
    }

    /// Wait for the pairing `start_pairing` opened to be approved, declined
    /// or to lapse. Approved, the app is signed in.
    pub async fn await_pairing(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().await_pairing().await
    }

    /// Give up the pairing this device is waiting on.
    pub fn cancel_pairing(&self) {
        self.0.cancel_pairing()
    }

    /// The device waiting on `pair`, an id or the code it shows, on the
    /// signed-in server, and where it asked from.
    pub async fn pairing_info(self: Arc<Self>, pair: String) -> Result<PairingInfo, KoanError> {
        self.0.clone().pairing_info(pair).await
    }

    /// Sign the device waiting on `pair` in as this account. Answers with its
    /// name.
    pub async fn approve_pairing(self: Arc<Self>, pair: String) -> Result<String, KoanError> {
        self.0.clone().approve_pairing(pair).await
    }

    pub async fn decline_pairing(self: Arc<Self>, pair: String) -> Result<(), KoanError> {
        self.0.clone().decline_pairing(pair).await
    }

    /// The account's ListenBrainz connection on the signed-in koan server.
    /// `None` while it has none.
    pub async fn scrobbling_status(
        self: Arc<Self>,
    ) -> Result<Option<ScrobblingConnection>, KoanError> {
        self.0.clone().scrobbling_status().await
    }

    /// Connect the account's ListenBrainz with its user token. The server
    /// checks it with ListenBrainz; a refusal is a `BadArgument` whose
    /// message can be shown as it is.
    pub async fn connect_scrobbling(
        self: Arc<Self>,
        token: String,
    ) -> Result<Option<ScrobblingConnection>, KoanError> {
        self.0.clone().connect_scrobbling(token).await
    }

    pub async fn disconnect_scrobbling(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().disconnect_scrobbling().await
    }

    /// Read a pairing link (`koan.rocks/pair/#s=…&p=…`). `None` for anything
    /// else.
    pub fn parse_pairing_link(&self, link: String) -> Option<PairingLink> {
        self.0.parse_pairing_link(link)
    }

    /// The server's accounts, on a server that lists `koanInvite`: accounts
    /// and their invites arrived together, and making one answers with its
    /// invite. Fails for an account that is not an admin, which is how the
    /// app knows to offer none of this.
    pub async fn server_accounts(self: Arc<Self>) -> Result<Vec<ServerAccount>, KoanError> {
        self.0.clone().server_accounts().await
    }

    /// Make an account with a generated password; its invite comes back.
    pub async fn create_server_account(
        self: Arc<Self>,
        username: String,
        role: AccountRole,
    ) -> Result<Invite, KoanError> {
        self.0.clone().create_server_account(username, role).await
    }

    /// An invite for an existing account. `reset` also gives it a new
    /// password, which comes back in the invite, signing its devices out.
    pub async fn invite_server_account(
        self: Arc<Self>,
        username: String,
        reset: bool,
    ) -> Result<Invite, KoanError> {
        self.0.clone().invite_server_account(username, reset).await
    }

    pub async fn set_server_account_role(
        self: Arc<Self>,
        username: String,
        role: AccountRole,
    ) -> Result<(), KoanError> {
        self.0.clone().set_server_account_role(username, role).await
    }

    /// Where an assistant connects to the signed-in server.
    pub async fn assistants(self: Arc<Self>) -> Result<Assistants, KoanError> {
        self.0.clone().assistants().await
    }

    /// The signed-in account's API keys.
    pub async fn api_keys(self: Arc<Self>) -> Result<Vec<ApiKeyInfo>, KoanError> {
        self.0.clone().api_keys().await
    }

    /// Make an API key for another app. The key is in the answer and nowhere
    /// else, ever.
    pub async fn create_api_key(self: Arc<Self>, name: String) -> Result<NewApiKey, KoanError> {
        self.0.clone().create_api_key(name).await
    }

    /// Revoke one of the account's keys. Not this device's own: that is
    /// signing out.
    pub async fn revoke_api_key(self: Arc<Self>, id: i64) -> Result<(), KoanError> {
        self.0.clone().revoke_api_key(id).await
    }

    /// The signed-in account's app passwords.
    pub async fn app_passwords(self: Arc<Self>) -> Result<Vec<AppPasswordInfo>, KoanError> {
        self.0.clone().app_passwords().await
    }

    /// Make an app password for a Subsonic app that signs in only with a
    /// token. The password is in the answer and nowhere else, ever.
    pub async fn create_app_password(
        self: Arc<Self>,
        name: String,
    ) -> Result<NewAppPassword, KoanError> {
        self.0.clone().create_app_password(name).await
    }

    /// Revoke one of the account's app passwords.
    pub async fn revoke_app_password(self: Arc<Self>, id: i64) -> Result<(), KoanError> {
        self.0.clone().revoke_app_password(id).await
    }

    /// Give another account a password. Its devices sign out.
    pub async fn set_server_account_password(
        self: Arc<Self>,
        username: String,
        password: String,
    ) -> Result<(), KoanError> {
        self.0
            .clone()
            .set_server_account_password(username, password)
            .await
    }

    /// Change the signed-in account's own password. This device stays signed
    /// in, with a new key; the account's other devices sign out.
    pub async fn change_own_password(
        self: Arc<Self>,
        current: String,
        password: String,
    ) -> Result<(), KoanError> {
        self.0.clone().change_own_password(current, password).await
    }

    pub async fn delete_server_account(self: Arc<Self>, username: String) -> Result<(), KoanError> {
        self.0.clone().delete_server_account(username).await
    }

    /// Forget the server and everything this device held for it: its
    /// library, the account's favourites, history and playlists, the queue
    /// and the downloads (`helpers::sign_out`). Local files stay.
    pub async fn sign_out_remote(self: Arc<Self>) -> Result<(), KoanError> {
        self.0.clone().sign_out_remote().await
    }

    /// Forget every track that came from a folder.
    ///
    /// A track the server also has keeps its row and loses only its local path.
    /// Albums and artists left holding nothing go too.
    pub async fn forget_folder(self: Arc<Self>, path: String) -> Result<u64, KoanError> {
        self.0.clone().forget_folder(path).await
    }

    /// Read the whole library again from its sources: the next scan re-reads
    /// every file and the next sync walks the whole server, and tracks take
    /// what their tags say now. Rows stay and their sources claim them back,
    /// so play history, playlists, favourites and lyrics are kept; a row
    /// nothing claims again goes. See `koan_core::helpers::rebuild_index`.
    pub async fn rebuild_index(self: Arc<Self>) -> Result<RebuildSummary, KoanError> {
        self.0.clone().rebuild_index().await
    }

    /// The download cache's limit in bytes, as koan reads `cache_limit`, or
    /// `None` when there is none. For drawing usage against it without a
    /// second parser of the setting on each platform.
    pub async fn cache_limit_bytes(self: Arc<Self>) -> Option<u64> {
        self.0.clone().cache_limit_bytes().await
    }

    /// Delete every downloaded remote track. The library rows stay.
    pub async fn clear_download_cache(self: Arc<Self>) -> Result<CacheCleared, KoanError> {
        self.0.clone().clear_download_cache().await
    }

    /// Delete the downloaded copies of just these tracks. The library rows
    /// stay, and they fetch again on demand.
    pub async fn clear_downloads(
        self: Arc<Self>,
        track_ids: Vec<i64>,
    ) -> Result<CacheCleared, KoanError> {
        self.0.clone().clear_downloads(track_ids).await
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
        self.0.clone().download_to_cache(track_ids).await
    }

    /// Stop fetching these tracks. Queue entries waiting on them fail as
    /// cancelled and are skipped until retried; a transfer running stops and
    /// leaves no row.
    pub async fn cancel_downloads(self: Arc<Self>, track_ids: Vec<i64>) -> Result<(), KoanError> {
        self.0.clone().cancel_downloads(track_ids).await
    }

    /// Fetch these tracks again after a failure or a cancel: queue entries
    /// that failed for them wait again, and a track no entry wants is
    /// fetched into the cache.
    pub async fn retry_downloads(self: Arc<Self>, track_ids: Vec<i64>) -> Result<(), KoanError> {
        self.0.clone().retry_downloads(track_ids).await
    }

    /// Rescans every configured library folder, saying how far it has got.
    /// Minutes, on a large library.
    pub async fn scan_reporting(
        self: Arc<Self>,
        force: bool,
        reporter: Option<Arc<dyn ProgressReporter>>,
    ) -> Result<ScanSummary, KoanError> {
        self.0
            .clone()
            .scan_reporting(
                force,
                reporter.map(|x| {
                    Arc::new(ProgressReporterBridge(x)) as Arc<dyn koan_runtime::ProgressReporter>
                }),
            )
            .await
    }

    /// Pull the remote library into the local database: walk it, then
    /// reconcile favourites and playlists. Long and network-bound. What the
    /// Sync button does — koan's own syncs go by `Walk::IfChanged`.
    pub async fn sync_remote(self: Arc<Self>) -> Result<SyncSummary, KoanError> {
        self.0.clone().sync_remote().await
    }

    /// The library of a server just signed in to. Walked only if the server's
    /// has moved since the last complete walk, which on a first sign-in it
    /// has: signing in also starts koan's own sync, and when that one has
    /// walked it first and recorded the server's version, this one waits for
    /// it and does not walk it again.
    pub async fn load_remote_library(self: Arc<Self>) -> Result<SyncSummary, KoanError> {
        self.0.clone().load_remote_library().await
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
        self.0.clone().create_share(track_ids, description).await
    }

    /// Track IDs for an album or an artist, in running order. What the context
    /// menu actions resolve to before touching the queue.
    pub async fn track_ids(
        self: Arc<Self>,
        album_id: Option<i64>,
        artist_id: Option<i64>,
    ) -> Result<Vec<i64>, KoanError> {
        self.0.clone().track_ids(album_id, artist_id).await
    }

    /// The tracks `sources` name, as `queue` resolves them: for something
    /// that wants the list itself, such as a new playlist's contents.
    pub async fn track_ids_of(
        self: Arc<Self>,
        sources: Vec<TrackSet>,
    ) -> Result<Vec<i64>, KoanError> {
        self.0.clone().track_ids_of(sources).await
    }

    /// Track IDs for several records, each in running order and the records
    /// in the order given. One read for a section's worth of albums.
    pub async fn album_track_ids(
        self: Arc<Self>,
        album_ids: Vec<i64>,
    ) -> Result<Vec<i64>, KoanError> {
        self.0.clone().album_track_ids(album_ids).await
    }

    /// Named patterns from `[organize.patterns]`, sorted by name.
    ///
    /// Patterns are config, shared with the CLI and TUI, so this reads them
    /// rather than offering somewhere else to define them.
    pub async fn organize_patterns(self: Arc<Self>) -> Vec<OrganizePattern> {
        self.0.clone().organize_patterns().await
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
        self.0.clone().save_organize_pattern(name, pattern).await
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
        self.0
            .clone()
            .organize_selection(track_ids)
            .await
            .map(|x| Arc::new(OrganizeSelection(x)))
    }

    /// Whether cover art, cue sheets and logs travel with the music.
    pub async fn organize_moves_ancillary(self: Arc<Self>) -> bool {
        self.0.clone().organize_moves_ancillary().await
    }

    /// Remember the choice. Written to `config.toml`, so the CLI and TUI
    /// organize the same way the app just did.
    pub async fn set_organize_moves_ancillary(
        self: Arc<Self>,
        enabled: bool,
    ) -> Result<(), KoanError> {
        self.0.clone().set_organize_moves_ancillary(enabled).await
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
        self.0
            .clone()
            .organize_execute(pattern, track_ids, base_dir)
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
        self.0.clone().import_files(paths).await
    }

    /// Where the library folders point. Shown in settings.
    pub async fn library_folders(self: Arc<Self>) -> Vec<String> {
        self.0.clone().library_folders().await
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
pub struct OrganizeSelection(pub(crate) Arc<koan_runtime::OrganizeSelection>);

#[uniffi::export]
impl OrganizeSelection {
    /// Turn the pattern into destinations. **Touches no files** — the cost is
    /// a destination per track, which is why it is off-thread like everything
    /// else rather than resolved inline as someone types.
    pub async fn generate(self: Arc<Self>, pattern: String, base_dir: String) -> OrganizePlan {
        self.0.clone().generate(pattern, base_dir).await
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
        self.0
            .clone()
            .check(pattern, base_dir, move_ancillary)
            .await
    }

    /// How many files resolved to something local. Fewer than were asked for
    /// means the rest are remote-only or gone from disk.
    pub fn count(&self) -> u32 {
        self.0.count()
    }
}

mod serve {
    #[allow(unused_imports)]
    use super::*;
    #[allow(unused_imports)]
    use koan_runtime::{
        AccountRole, Album, AlbumOnDevice, AlbumPage, AlbumSort, ApiKeyInfo, AppPasswordInfo,
        Appearance, ArtEvictions, Artist, ArtistInfo, Assistants, AttestedKey, AutoEqEntry,
        AutoEqMaker, AutoEqOffer, BrowseChoices, BrowseFilter, CacheCleared, CommandNotice,
        ConnectedInfo, ConnectionInfo, CoverArt, CoverPreload, CreditPart, DeletedDsp, Device,
        DeviceInfo, DeviceReach, DspBand, DspBandBounds, DspBandKind, DspBrush, DspChannelLayout,
        DspCurve, DspDeviceAction, DspEarKind, DspEdited, DspEqJoin, DspImportPlan,
        DspImportRefusal, DspImportSummary, DspImpulse, DspInfo, DspJoin, DspLayerCurve,
        DspLayerInfo, DspMadeAgainstPreview, DspMeasurementReading, DspMixOutput, DspMixSource,
        DspOutputState, DspOverview, DspPoint, DspProfileDetail, DspProfileSummary, DspResponse,
        DspRole, DspSavedCopy, DspStarter, DspTargetName, DspTargetOption, DspTargets,
        DspTuningEntry, EntryStatus, FuzzyMatch, HeldInfo, ImportSummary, InfoField, Invite,
        InviteTo, KoanError, LibraryFolder, LyricLine, Lyrics, MixerEncoding, MixerFormat,
        MoveResult, NearbyServer, NewApiKey, NewAppPassword, NowPlaying, OrganizeEntry,
        OrganizePattern, OrganizePlan, OutputChoice, OutputInfo, OutputsInfo, PairingCode,
        PairingInfo, PairingLink, Placement, PlanOutcome, PlayHistoryEntry, PlayState, Playlist,
        PlaylistEntry, PlaylistExport, QrCode, QueueItem, QueueLock, RebuildSummary,
        RecentlyPlayed, ReleaseSection, RendererInfo, RendererOutput, RepeatMode, RunRole,
        ScanSummary, ScrobblingConnection, SearchKind, ServerAccount, ServerExtension, ServerQueue,
        ServerStanding, Settings, Share, ShareStatus, Sharing, ShelfKind, ShelfSummary, SleepState,
        SleepTimer, SquigHit, StateSlice, Stats, StreamFormat, SyncPhase, SyncProgress,
        SyncSummary, TailscaleStatus, TopShelf, TopShelfRecord, Track, TrackBrowseSort,
        TrackDetails, TrackListing, TrackSet, TrackSource, Transfer, TransferFigure, TransferState,
        TxtEntry, VizLevels,
    };

    /// Where the library is being served, while it is.
    #[uniffi::remote(Record)]
    pub struct Sharing {
        pub port: u16,
        /// Where a phone on this network reaches it, e.g.
        /// `http://192.168.1.20:4000`. `None` with no network to be reached on.
        pub lan_url: Option<String>,
    }

    /// Tailscale on this device, for Settings.
    #[uniffi::remote(Record)]
    pub struct TailscaleStatus {
        /// The device's name on the tailnet, `mac.tailnet.ts.net`.
        pub name: String,
        /// The tailnet lets this device use Funnel.
        pub funnel_allowed: bool,
    }

    /// Where invites send phones.
    #[uniffi::remote(Enum)]
    pub enum InviteTo {
        /// The device's address on the local network.
        Network,
        /// Its Tailscale name, for phones with Tailscale, anywhere.
        Tailnet,
        /// Its address on the internet through Tailscale Funnel, published
        /// whenever it serves.
        Funnel,
        /// A tunnel's address, typed in Settings.
        Custom,
    }

    /// Where invites go and the addresses Settings shows, as the engine works
    /// them out, read together so the page never mixes two moments.
    #[uniffi::remote(Record)]
    pub struct ShareStatus {
        pub to: InviteTo,
        /// Tailscale on this device, while it is running and signed in.
        pub tailscale: Option<TailscaleStatus>,
        /// Where invites send phones now. The addresses are `None` while not
        /// serving.
        pub current: Option<String>,
        /// Where they go with no custom address typed: the tailnet's, else the
        /// network's.
        pub fallback: Option<String>,
        /// Where Funnel publishes the library, while it does.
        pub funnel: Option<String>,
    }

    #[uniffi::export]
    impl KoanEngine {
        /// Serve the library on every interface, on `port` (0 for any free one).
        /// `names` are what else the server answers to as a host, such as this
        /// Mac's Bonjour name. A server already running is stopped first, so a
        /// start that fails, on a port taken, leaves nothing serving.
        pub async fn start_server(
            self: Arc<Self>,
            port: u16,
            names: Vec<String>,
        ) -> Result<Sharing, KoanError> {
            self.0.clone().start_server(port, names).await
        }

        /// Stop serving, closing every open connection. Nothing if no server
        /// runs. Can take as long as the server's drain; nothing waits on it at
        /// quit, when the process closing does the same.
        pub async fn stop_server(self: Arc<Self>) {
            self.0.clone().stop_server().await
        }

        /// Where the library is served, while it is, with the network address
        /// worked out afresh: it moves when the Mac changes network.
        pub async fn server_sharing(self: Arc<Self>) -> Option<Sharing> {
            self.0.clone().server_sharing().await
        }

        /// Where invites go, Tailscale, and the addresses Settings shows.
        pub async fn share_status(self: Arc<Self>) -> ShareStatus {
            self.0.clone().share_status().await
        }

        /// Start taking down the Funnel kōan published, as the app quits: the
        /// server is not stopped then, so nothing else would. Returns at once;
        /// the next start publishes it again where it is wanted.
        pub fn release_funnel(&self) {
            self.0.release_funnel()
        }

        /// Where invites send phones (`share.invite_to`). `Funnel` publishes the
        /// library on the internet through Tailscale Funnel, now and whenever it
        /// is served; any other choice stops publishing. `Custom` sends them to
        /// `address` (`share.url`), kept as it was when `None`; any other choice
        /// clears it, since the server tells phones it is the address it is known
        /// by.
        pub async fn set_invite_to(
            self: Arc<Self>,
            to: InviteTo,
            address: Option<String>,
        ) -> Result<(), KoanError> {
            self.0.clone().set_invite_to(to, address).await
        }

        /// Give the account the library is shared under a new password. This
        /// device is the server's host, so the old one is not asked for. Signs
        /// the account's other devices out.
        pub async fn reset_server_admin_password(
            self: Arc<Self>,
            password: String,
        ) -> Result<(), KoanError> {
            self.0.clone().reset_server_admin_password(password).await
        }

        /// The account the library is shared under, its first admin: the one
        /// this device administers the server as. `None` until one is made, and
        /// sharing starts only once there is one, since nobody could sign in.
        pub async fn server_admin(self: Arc<Self>) -> Result<Option<String>, KoanError> {
            self.0.clone().server_admin().await
        }

        /// Make the first account, an admin, in this library's database: the one
        /// sharing begins with, and the one this device administers it as.
        /// Refused once any account exists, with the server's own rules for names
        /// and passwords.
        pub async fn create_first_server_account(
            self: Arc<Self>,
            username: String,
            password: String,
        ) -> Result<(), KoanError> {
            self.0
                .clone()
                .create_first_server_account(username, password)
                .await
        }
    }
}
#[allow(unused_imports)]
pub use serve::*;

mod state {
    #[allow(unused_imports)]
    use super::*;
    #[allow(unused_imports)]
    use koan_runtime::{
        AccountRole, Album, AlbumOnDevice, AlbumPage, AlbumSort, ApiKeyInfo, AppPasswordInfo,
        Appearance, ArtEvictions, Artist, ArtistInfo, Assistants, AttestedKey, AutoEqEntry,
        AutoEqMaker, AutoEqOffer, BrowseChoices, BrowseFilter, CacheCleared, CommandNotice,
        ConnectedInfo, ConnectionInfo, CoverArt, CoverPreload, CreditPart, DeletedDsp, Device,
        DeviceInfo, DeviceReach, DspBand, DspBandBounds, DspBandKind, DspBrush, DspChannelLayout,
        DspCurve, DspDeviceAction, DspEarKind, DspEdited, DspEqJoin, DspImportPlan,
        DspImportRefusal, DspImportSummary, DspImpulse, DspInfo, DspJoin, DspLayerCurve,
        DspLayerInfo, DspMadeAgainstPreview, DspMeasurementReading, DspMixOutput, DspMixSource,
        DspOutputState, DspOverview, DspPoint, DspProfileDetail, DspProfileSummary, DspResponse,
        DspRole, DspSavedCopy, DspStarter, DspTargetName, DspTargetOption, DspTargets,
        DspTuningEntry, EntryStatus, FuzzyMatch, HeldInfo, ImportSummary, InfoField, Invite,
        InviteTo, KoanError, LibraryFolder, LyricLine, Lyrics, MixerEncoding, MixerFormat,
        MoveResult, NearbyServer, NewApiKey, NewAppPassword, NowPlaying, OrganizeEntry,
        OrganizePattern, OrganizePlan, OutputChoice, OutputInfo, OutputsInfo, PairingCode,
        PairingInfo, PairingLink, Placement, PlanOutcome, PlayHistoryEntry, PlayState, Playlist,
        PlaylistEntry, PlaylistExport, QrCode, QueueItem, QueueLock, RebuildSummary,
        RecentlyPlayed, ReleaseSection, RendererInfo, RendererOutput, RepeatMode, RunRole,
        ScanSummary, ScrobblingConnection, SearchKind, ServerAccount, ServerExtension, ServerQueue,
        ServerStanding, Settings, Share, ShareStatus, Sharing, ShelfKind, ShelfSummary, SleepState,
        SleepTimer, SquigHit, StateSlice, Stats, StreamFormat, SyncPhase, SyncProgress,
        SyncSummary, TailscaleStatus, TopShelf, TopShelfRecord, Track, TrackBrowseSort,
        TrackDetails, TrackListing, TrackSet, TrackSource, Transfer, TransferFigure, TransferState,
        TxtEntry, VizLevels,
    };

    /// One slice of engine state, whole.
    // Boxing is what clippy wants for the size spread and is not on offer across
    // the FFI. The saving would be nothing anyway: a handful of these exist at a
    // time, built a few times a second, never held in a collection.
    #[uniffi::remote(Enum)]
    pub enum StateSlice {
        /// Everything a transport bar shows other than the position. `position_ms`
        /// is always zero here — it has a slice of its own, and leaving it in would
        /// make every tick a change to this one.
        Playback { now_playing: NowPlaying },
        /// Where the playhead is, how far it can be dragged, and whether it is
        /// moving on its own.
        ///
        /// An anchor, not a reading. A client that knows the playhead was at
        /// `position_ms` when this arrived, and that it is playing, knows where it
        /// is now without being told again — so this is published when that stops
        /// being true (a seek, a pause, a new track, a stall) rather than on a
        /// clock. It is the one number in the engine that changes without anything
        /// happening, and a stream that had to keep saying so could never be quiet.
        ///
        /// One slice because they move together and are read by one thing. The
        /// seekable extent belongs here rather than beside the track's title: it
        /// grows for as long as a download is arriving, and a field that moves at
        /// that rate drags every reader of whatever it sits next to along with it.
        Playhead {
            position_ms: u64,
            seekable_ms: u64,
            playing: bool,
        },
        /// The queue, in order. The whole list: a queue that is one copy refetched
        /// whole has nothing to forget to patch.
        ///
        /// The version travels with the rows rather than in `Playback`: a client
        /// that wants to know the queue moved is reading the queue, and one that
        /// is drawing a transport bar is not — leaving it there re-ran every
        /// reader of what is playing on every edit to what is queued.
        ///
        /// Sent when the queue's content changes, not when its rows' statuses do:
        /// those arrive as `QueuePatch`, so a queue of tens of thousands of rows
        /// is not sent again for every track change and download.
        Queue { items: Vec<QueueItem>, version: u64 },
        /// Every row of the `Queue` whose `version` is `base` that reads
        /// differently now — a status, a duration, a failure, where its bytes are
        /// — whole. Relative to the base rather than to the last patch, so the
        /// latest one is all a client needs: put the rows of the last one it
        /// applied back as the base had them, and apply this. A patch for any
        /// other base is stale and ignored; the queue it described has been sent
        /// whole since.
        QueuePatch { base: u64, items: Vec<QueueItem> },
        /// What the queue still is, when it is still a playlist or a record.
        Lock { lock: Option<QueueLock> },
        /// Every transfer koan knows about — running first, then whatever settled
        /// most recently. Structural only; see `Figures`.
        Transfers { transfers: Vec<Transfer> },
        /// The byte counts behind those transfers. The fast half.
        Figures { figures: Vec<TransferFigure> },
        /// The library's rows changed — a scan, a sync, an import, an organize, a
        /// download landing, a folder being forgotten.
        ///
        /// A version, not a mirror. A record's tracks and a search's results are
        /// asked for on demand; sending every album's tracks across the boundary
        /// so a client can hold a copy is the opposite of the point. What a client
        /// needs is to know to ask again.
        Library { version: u64 },
        /// Moves whenever the play history does. Pages derived from it ask again.
        History { version: u64 },
        /// Bytes in the download cache, as downloads land, trims remove them and
        /// clears empty it.
        Cache { bytes: u64 },
        /// What koan is doing on its own: the startup and watched-folder scan, and
        /// the automatic sync with a server.
        ///
        /// Flags rather than a version, because a client draws a row per running
        /// task and nothing else. They flip twice per task, which is why they can
        /// be published rather than asked after.
        Tasks { scanning: bool, syncing: bool },
        /// How far the running remote sync has got, automatic or asked for.
        /// `None` when no sync is running.
        ///
        /// Apart from `Tasks` because it moves once per page of a sync — a few
        /// hundred times in a first sync of a large library — and a reader that
        /// only draws whether a task is running should not wake for each.
        Sync { progress: Option<SyncProgress> },
        /// How far fetching every record's cover ahead has got
        /// (`remote.preload_covers`). `None` when it is not running.
        Covers { progress: Option<CoverPreload> },
        /// The other devices koan can play on, and which one this app is
        /// controlling: `None` for this one. While it is another, `Playback`,
        /// `Playhead` and `Queue` describe that device rather than this one, so
        /// every page that shows what is playing shows it without knowing.
        ///
        /// `target_reach` is how the controlled device can be reached, from its
        /// connections rather than a failed command, so a client can mark it out
        /// of reach before anything is pressed. `None` while this one is in view.
        Devices {
            devices: Vec<DeviceInfo>,
            target: Option<String>,
            target_reach: Option<DeviceReach>,
        },
        /// The server and the network, as far as devices are concerned.
        Connection { connection: ConnectionInfo },
        /// UPnP renderers on the network, and the one this koan is playing to.
        /// Unlike `Devices`, a renderer is this device's output: the transport and
        /// queue stay this device's own while one plays.
        Renderers {
            renderers: Vec<RendererInfo>,
            output: Option<RendererOutput>,
            /// The last search for renderers was refused the local network: on
            /// macOS, the app has not been allowed it, and none can be found.
            local_network_blocked: bool,
        },
        /// What the device in view plays through: this one, or the one being
        /// controlled. `None` while a controlled device has not said.
        Outputs { outputs: Option<OutputsInfo> },
    }

    /// One client's place in the state stream.
    ///
    /// A cursor, not a subscription queue. A broadcast channel loses whatever was
    /// published while the client was away.
    /// A cursor cannot lose a change. It can only arrive later, holding a newer
    /// value, which is what every slice being a whole snapshot makes safe.
    #[derive(uniffi::Object)]
    pub struct StateStream(pub(crate) Arc<koan_runtime::StateStream>);

    #[uniffi::export]
    impl StateStream {
        /// Everything that has changed since this stream last answered, at most one
        /// message per slice.
        ///
        /// Waits when nothing has. `None` once the engine is gone, which ends the
        /// caller's loop.
        pub async fn next(&self) -> Option<Vec<StateSlice>> {
            self.0.next().await
        }
    }
}
#[allow(unused_imports)]
pub use state::*;

mod types {
    #[allow(unused_imports)]
    use super::*;
    #[allow(unused_imports)]
    use koan_runtime::{
        AccountRole, Album, AlbumOnDevice, AlbumPage, AlbumSort, ApiKeyInfo, AppPasswordInfo,
        Appearance, ArtEvictions, Artist, ArtistInfo, Assistants, AttestedKey, AutoEqEntry,
        AutoEqMaker, AutoEqOffer, BrowseChoices, BrowseFilter, CacheCleared, CommandNotice,
        ConnectedInfo, ConnectionInfo, CoverArt, CoverPreload, CreditPart, DeletedDsp, Device,
        DeviceInfo, DeviceReach, DspBand, DspBandBounds, DspBandKind, DspBrush, DspChannelLayout,
        DspCurve, DspDeviceAction, DspEarKind, DspEdited, DspEqJoin, DspImportPlan,
        DspImportRefusal, DspImportSummary, DspImpulse, DspInfo, DspJoin, DspLayerCurve,
        DspLayerInfo, DspMadeAgainstPreview, DspMeasurementReading, DspMixOutput, DspMixSource,
        DspOutputState, DspOverview, DspPoint, DspProfileDetail, DspProfileSummary, DspResponse,
        DspRole, DspSavedCopy, DspStarter, DspTargetName, DspTargetOption, DspTargets,
        DspTuningEntry, EntryStatus, FuzzyMatch, HeldInfo, ImportSummary, InfoField, Invite,
        InviteTo, KoanError, LibraryFolder, LyricLine, Lyrics, MixerEncoding, MixerFormat,
        MoveResult, NearbyServer, NewApiKey, NewAppPassword, NowPlaying, OrganizeEntry,
        OrganizePattern, OrganizePlan, OutputChoice, OutputInfo, OutputsInfo, PairingCode,
        PairingInfo, PairingLink, Placement, PlanOutcome, PlayHistoryEntry, PlayState, Playlist,
        PlaylistEntry, PlaylistExport, QrCode, QueueItem, QueueLock, RebuildSummary,
        RecentlyPlayed, ReleaseSection, RendererInfo, RendererOutput, RepeatMode, RunRole,
        ScanSummary, ScrobblingConnection, SearchKind, ServerAccount, ServerExtension, ServerQueue,
        ServerStanding, Settings, Share, ShareStatus, Sharing, ShelfKind, ShelfSummary, SleepState,
        SleepTimer, SquigHit, StateSlice, Stats, StreamFormat, SyncPhase, SyncProgress,
        SyncSummary, TailscaleStatus, TopShelf, TopShelfRecord, Track, TrackBrowseSort,
        TrackDetails, TrackListing, TrackSet, TrackSource, Transfer, TransferFigure, TransferState,
        TxtEntry, VizLevels,
    };

    #[uniffi::remote(Enum)]
    pub enum PlayState {
        Stopped,
        Playing,
        Paused,
    }

    #[uniffi::remote(Enum)]
    pub enum EntryStatus {
        Queued,
        Playing,
        Played,
        Downloading,
        PriorityPending,
        Failed,
    }

    /// This device and the account's play queue on the server.
    #[uniffi::remote(Record)]
    pub struct ServerQueue {
        /// This device keeps its queue there.
        pub on: bool,
        /// How many tracks the server holds; 0 when it has none.
        pub saved_tracks: u32,
        /// Who saved it, as a person would name them.
        pub saved_by: String,
    }

    #[uniffi::remote(Record)]
    pub struct Artist {
        pub id: i64,
        pub name: String,
        pub sort_name: Option<String>,
        pub album_count: i64,
        pub track_count: i64,
    }

    /// One stretch of an artist credit: a credited artist's name, linked to that
    /// artist, or the plain text joining two names. A credit's parts, in order,
    /// read as the credit does, or as its artists joined by commas where a name
    /// could not be found in it. See `koan_core::helpers::credit_parts`.
    #[uniffi::remote(Record)]
    pub struct CreditPart {
        pub text: String,
        pub artist_id: Option<i64>,
    }

    /// A row's place among the records of a list. See
    /// `koan_core::helpers::RunRole`: a heading goes above each `Opens` row,
    /// drawn from that row's record.
    #[uniffi::remote(Enum)]
    pub enum RunRole {
        Alone,
        Single,
        Opens,
        Continues,
    }

    #[uniffi::remote(Record)]
    pub struct Album {
        pub id: i64,
        pub title: String,
        pub artist_id: i64,
        pub artist_name: String,
        /// `artist_name` with each artist it credits linked.
        pub artist_credit: Vec<CreditPart>,
        pub year: Option<i32>,
        pub codec: Option<String>,
        pub label: Option<String>,
        pub total_discs: Option<i32>,
        pub total_tracks: Option<i32>,
        /// When it entered the library. Sortable text — the server's ISO `created`
        /// for remote albums, SQLite's `datetime('now')` for locally scanned ones.
        pub added_at: Option<String>,
        /// How much of it can play here: set where a listing is narrowed to this
        /// device, the Downloaded shelf and offline.
        pub on_device: Option<AlbumOnDevice>,
        /// What kind of release it is, as its tile shows it (`EP`, `Live`).
        /// Absent for an album, and where nobody tagged the type.
        pub release_badge: Option<String>,
        /// The heading it sits under on its artist's page.
        pub release_section: ReleaseSection,
    }

    /// The sections of an artist's page, in page order. Titled by
    /// `release_section_title`.
    #[uniffi::remote(Enum)]
    pub enum ReleaseSection {
        Albums,
        EpsAndSingles,
        Live,
        Compilations,
        Other,
    }

    /// Of a record's tracks, how many can play here.
    #[uniffi::remote(Record)]
    pub struct AlbumOnDevice {
        pub have: u32,
        pub total: u32,
    }

    #[uniffi::remote(Record)]
    pub struct Track {
        pub id: i64,
        pub title: String,
        pub artist_name: String,
        /// `artist_name` with each artist it credits linked.
        pub artist_credit: Vec<CreditPart>,
        pub album_artist_name: String,
        /// The record's artist, and its credit with each artist it names linked:
        /// what a heading or page over the record draws.
        pub album_artist_id: Option<i64>,
        pub album_artist_credit: Vec<CreditPart>,
        /// The track's credit reads differently from its record's: under the
        /// record's heading or on its page, a row names its artists only when this
        /// is set. See `koan_core::helpers::RowCredits::differ`.
        pub credit_differs: bool,
        pub album_title: String,
        pub album_id: Option<i64>,
        pub artist_id: Option<i64>,
        pub disc: Option<i32>,
        pub track_number: Option<i32>,
        pub duration_ms: Option<i64>,
        pub codec: Option<String>,
        pub sample_rate: Option<i32>,
        pub bit_depth: Option<i32>,
        pub channels: Option<i32>,
        pub bitrate: Option<i32>,
        pub genre: Option<String>,
        /// `"local"` or `"remote"` — remote tracks download on demand.
        pub source: String,
        /// The server knows about this track. Independent of `on_disk`: a record
        /// held both locally and on the server is one row that is both, and a UI
        /// that treats source as a single value cannot say so.
        pub on_server: bool,
        /// The bytes are on this machine — an indexed file or a finished download.
        pub on_disk: bool,
        /// Present once the file exists on disk, locally or in the cache.
        pub path: Option<String>,
        pub is_favourite: bool,
    }

    /// Everything known about one track, for an info view: the track as lists
    /// show it, what each of its sources says, and the ReplayGain in its file.
    #[uniffi::remote(Record)]
    pub struct TrackDetails {
        pub track: Track,
        pub uid: Option<String>,
        /// The file's first, then the server's.
        pub sources: Vec<TrackSource>,
        /// The ReplayGain tags of the file on this device, when there is one.
        pub replay_gain: Vec<InfoField>,
    }

    /// One source of a track, as a list of what it says, empty values left out.
    #[uniffi::remote(Record)]
    pub struct TrackSource {
        /// `"file"` or `"server"`.
        pub kind: String,
        pub fields: Vec<InfoField>,
    }

    #[uniffi::remote(Record)]
    pub struct InfoField {
        pub name: String,
        pub value: String,
    }

    /// What was played lately, each once and newest first by its latest play.
    #[uniffi::remote(Record)]
    pub struct RecentlyPlayed {
        pub albums: Vec<Album>,
        pub artists: Vec<Artist>,
        pub tracks: Vec<Track>,
    }

    /// What the Apple TV's Top Shelf shows: the records played lately and the
    /// favourite records, in the order their shelf pages list them.
    #[uniffi::remote(Record)]
    pub struct TopShelf {
        pub recent: Vec<TopShelfRecord>,
        pub favourites: Vec<TopShelfRecord>,
    }

    /// A record on the Top Shelf, with the uid its link names it by: the shelf is
    /// read while koan is not running, and a row id may by then be another row's.
    #[uniffi::remote(Record)]
    pub struct TopShelfRecord {
        pub album: Album,
        pub uid: String,
    }

    /// Which shelf: see `koan_core::shelves::Shelf`.
    #[uniffi::remote(Enum)]
    pub enum ShelfKind {
        Favourites,
        Recent,
        Search { query: String },
        Downloaded,
    }

    /// The first few of each kind on a shelf, and how many there are in all.
    #[uniffi::remote(Record)]
    pub struct ShelfSummary {
        pub artists: Vec<Artist>,
        pub artist_total: u64,
        pub albums: Vec<Album>,
        pub album_total: u64,
        pub tracks: Vec<Track>,
        pub track_total: u64,
    }

    /// One play, with the track it played.
    #[uniffi::remote(Record)]
    pub struct PlayHistoryEntry {
        /// Identifies this play, not the track — the same track played twice is
        /// two entries with two ids.
        pub id: i64,
        pub track: Track,
        /// Unix seconds.
        pub played_at: i64,
        /// How long the track was listened to, where that was recorded.
        pub listened_ms: Option<i64>,
        /// `local` for koan's own playback, `subsonic` for a client scrobbling in.
        pub source: String,
    }

    /// Audio format of the track on the wire right now — what the DAC is actually
    /// being fed, as opposed to what the database claims.
    #[uniffi::remote(Record)]
    pub struct StreamFormat {
        pub codec: String,
        pub sample_rate: u32,
        pub bit_depth: Option<u16>,
        pub bitrate_kbps: Option<u32>,
        pub channels: u16,
        /// What the output device settled at, where it is known. Equal to
        /// `sample_rate` means koan handed the device the samples as they are;
        /// anything else means something resampled to reach it. What happens
        /// past the device — other clients, the volume stage — is the system's,
        /// and this says nothing about it.
        pub output_sample_rate: Option<u32>,
        /// Whether the system mixes the output with other apps' on its way to the
        /// device, where the platform says: on Android, `false` only while koan
        /// holds a USB DAC bit-perfect. `None` elsewhere, where
        /// `output_sample_rate` is the whole story.
        pub output_mixed: Option<bool>,
        /// What DSP is doing to the audio. `None` is the untouched path.
        pub dsp: Option<DspInfo>,
    }

    /// The output device's DSP profile, while one is running.
    #[uniffi::remote(Record)]
    pub struct DspInfo {
        pub profile: String,
        pub eq: bool,
        /// The rate the impulse response in use was designed at, which the output
        /// runs at. A source at another rate is resampled to reach it.
        pub convolution_rate: Option<u32>,
    }

    #[uniffi::remote(Record)]
    pub struct NowPlaying {
        pub state: PlayState,
        /// Waiting for the track under the cursor to arrive. With `Stopped` it
        /// opens playing, with `Paused` it opens paused.
        pub waiting: bool,
        pub position_ms: u64,
        pub duration_ms: u64,
        /// Queue item currently under the cursor, if any.
        pub queue_item_id: Option<String>,
        pub entry: Option<QueueItem>,
        pub format: Option<StreamFormat>,
        /// Bumped on every queue mutation — cheap change detection for the UI.
        pub playlist_version: u64,
        /// Shuffle is on: the next track is picked at random from those yet to
        /// play this pass. The queue keeps its order.
        pub shuffle: bool,
        pub repeat_mode: RepeatMode,
        /// How many shuffles of the queue undo would take back before reaching
        /// any other edit.
        pub undo_shuffles: u32,
        /// `shuffle_queue` has two rows or more to move: after the cursor, or
        /// anywhere when there is none.
        pub queue_shufflable: bool,
        /// The sleep timer, while one is set.
        pub sleep: Option<SleepState>,
        /// The sleep timer is fading playback out: not bit-perfect meanwhile.
        pub sleep_fading: bool,
        /// Why the last track asked for did not start, until one does: the
        /// output failing to open, a file that does not decode.
        pub start_failure: Option<String>,
    }

    /// A sleep timer to set: stop after a while, or at the end of the track or
    /// record playing.
    #[uniffi::remote(Enum)]
    pub enum SleepTimer {
        After { minutes: u32 },
        EndOfTrack,
        EndOfRecord,
    }

    /// A sleep timer that is set. A time rather than what is left, so a client
    /// counts down for itself.
    #[uniffi::remote(Enum)]
    pub enum SleepState {
        /// Milliseconds since the Unix epoch.
        At {
            unix_ms: u64,
        },
        EndOfTrack,
        EndOfRecord,
    }

    /// What follows a track at its end.
    #[uniffi::remote(Enum)]
    pub enum RepeatMode {
        Off,
        /// The last track runs on into the first.
        Queue,
        /// The track plays again. Next and previous still move on.
        One,
    }

    #[uniffi::remote(Record)]
    pub struct QueueItem {
        pub queue_item_id: String,
        pub track_id: Option<i64>,
        /// The record this came off, where the library still knows. Carried so a
        /// client can draw one sleeve per album rather than asking for artwork once
        /// per track and fetching the same image a dozen times.
        pub album_id: Option<i64>,
        /// The artist credited on the track and the record's artist, where the
        /// library knows them: where the names on the row lead.
        pub artist_id: Option<i64>,
        pub album_artist_id: Option<i64>,
        pub title: String,
        pub artist: String,
        pub album_artist: String,
        /// `artist` and `album_artist` with each artist they credit linked.
        pub artist_credit: Vec<CreditPart>,
        pub album_artist_credit: Vec<CreditPart>,
        /// The track's credit reads differently from its record's: grouped under
        /// the record's heading, the row names its artists only when this is set.
        /// See `koan_core::helpers::RowCredits::differ`.
        pub credit_differs: bool,
        /// Its place among the queue's records. `Alone` on `NowPlaying`'s entry,
        /// which is read outside the list.
        pub run: RunRole,
        pub album: String,
        pub year: Option<String>,
        pub codec: Option<String>,
        pub track_number: Option<i64>,
        pub disc: Option<i64>,
        pub duration_ms: Option<u64>,
        pub status: EntryStatus,
        /// The playlist row this came from, when it came from one. Survives the
        /// queue being shuffled or cut about — the queue is a view onto the
        /// playlist, not a copy of it.
        pub playlist_entry_id: Option<i64>,
        /// Why this item cannot play, when `status` is `Failed`.
        pub failure_reason: Option<String>,
        /// The server knows about this track. False for a queue item with no
        /// library row behind it, which has nowhere to have come from.
        pub on_server: bool,
        /// The bytes are on this machine — an indexed file or a finished download.
        pub on_disk: bool,
    }

    /// One transfer, as the download store has it — everything about it that does
    /// not move while the bytes land.
    ///
    /// The numbers are in `TransferFigure`, deliberately. A transfer appears,
    /// changes state a handful of times and settles; its byte count moves ten times
    /// a second for as long as it runs. Carrying both in one value would mean a
    /// list rebuilding at the rate a download writes.
    ///
    /// Identified by its track: a track is fetched once, however many queue entries
    /// want it, and a queue row finds its transfer by the track it plays.
    #[uniffi::remote(Record)]
    pub struct Transfer {
        pub track_id: i64,
        /// The record it belongs to, so a record's tile can say it is downloading.
        pub album_id: Option<i64>,
        pub artist_id: Option<i64>,
        pub title: String,
        pub artist: String,
        /// `artist` with each artist it credits linked.
        pub artist_credit: Vec<CreditPart>,
        /// The record's title, empty where the library has none.
        pub album: String,
        pub state: TransferState,
        /// Why it stopped, when it failed.
        pub failure_reason: Option<String>,
    }

    /// What one transfer is doing right now.
    ///
    /// The volatile half of a `Transfer`, split out so the two travel at their own
    /// rates. Serves both the figure on a queue row and the row on the downloads
    /// page — one reading of one fact, so they cannot disagree.
    #[uniffi::remote(Record)]
    pub struct TransferFigure {
        pub track_id: i64,
        /// 0.0–1.0, or `None` when the server sent no Content-Length — a bar drawn
        /// at zero for a transfer that is going fine reads as stuck.
        pub progress: Option<f64>,
        pub bytes_written: u64,
        pub total_bytes: u64,
        /// Smoothed. Zero for a transfer that has settled, and for one that has
        /// stopped moving — which is the case worth seeing.
        pub bytes_per_second: u64,
    }

    #[uniffi::remote(Enum)]
    pub enum TransferState {
        Queued,
        Running,
        Done,
        Failed,
    }

    /// A record and its tracks, as one answer.
    ///
    /// The page wants both and wants them together, so they are one call: one hop
    /// across the boundary, one connection out of the pool, one lock taken.
    #[uniffi::remote(Record)]
    pub struct AlbumPage {
        pub album: Option<Album>,
        pub tracks: Vec<Track>,
    }

    /// A created share link, and how much of the request it covers.
    ///
    /// `skipped` is the point of this being a record rather than a bare string: a
    /// selection mixing local-only files with server-backed ones produces a link
    /// that is partial, and the UI has to be able to say so.
    #[uniffi::remote(Record)]
    pub struct Share {
        pub url: String,
        pub shared: u32,
        pub skipped: u32,
    }

    #[uniffi::remote(Record)]
    pub struct Stats {
        pub total_tracks: i64,
        pub local_tracks: i64,
        pub remote_tracks: i64,
        pub cached_tracks: i64,
        pub total_albums: i64,
        pub total_artists: i64,
    }

    #[uniffi::remote(Record)]
    pub struct Device {
        pub name: String,
        pub sample_rates: Vec<f64>,
        /// How it is connected: `builtin`, `usb`, `bluetooth`, `airplay`,
        /// `display`, `virtual` or `other`. For its icon.
        pub kind: String,
    }

    /// Cover art as raw bytes. The GraphQL surface base64s this because JSON has to;
    /// across FFI it stays binary.
    #[uniffi::remote(Record)]
    pub struct CoverArt {
        pub data: Vec<u8>,
        pub mime: String,
    }

    #[uniffi::remote(Record)]
    pub struct LyricLine {
        pub time_secs: f64,
        pub text: String,
    }

    #[uniffi::remote(Record)]
    pub struct Lyrics {
        pub content: String,
        /// LRC with timestamps, as opposed to a plain text dump.
        pub synced: bool,
        /// Where the words came from, as a credit to show beside them: "LRCLIB",
        /// "Embedded" or "Sidecar". Absent when the cache names a source this
        /// build does not know.
        pub source: Option<String>,
        /// Parsed LRC lines, empty when unsynced. Parsing lives here so clients
        /// don't each reimplement the timestamp format.
        pub lines: Vec<LyricLine>,
    }

    #[uniffi::remote(Record)]
    pub struct Playlist {
        pub id: i64,
        pub name: String,
        pub comment: Option<String>,
        /// Set once the playlist exists on the server. Its absence is what tells a
        /// client the playlist is local-only.
        pub remote_id: Option<String>,
        pub public: bool,
        pub owner: Option<String>,
        pub track_count: u32,
        pub duration_ms: i64,
        pub created_at: String,
        pub changed_at: String,
        /// How this machine likes to look at it. `None` follows the app default.
        pub grouped: Option<bool>,
        /// Its contents are not for editing: rules or a playlist file decide
        /// them, here or on the server. Adds, removals and reorders are refused.
        pub readonly: bool,
        /// Rules here decide its contents (a smart playlist on this machine,
        /// rather than one mirrored from a server).
        pub smart: bool,
        /// Read from a file in the library, which decides its name and contents.
        pub from_file: bool,
    }

    /// What the queue still is, when it is still something.
    ///
    /// A queue that came from a playlist or a record and has not been touched since
    /// is still that thing, and saying so is what makes following legible — you can
    /// see why an edit to the playlist moved something in the queue, and you can
    /// see the moment it stops.
    #[uniffi::remote(Enum)]
    pub enum QueueLock {
        Playlist { playlist: Playlist },
        Album { album: Album },
    }

    /// One row of a playlist: the track, and the entry it sits in.
    ///
    /// `entry_id` is the entry's own, not the track's: it is what a queue item
    /// remembers, so a client can tell which of two copies of a song is the one
    /// playing. Named so that a list of them cannot be read as track ids.
    #[uniffi::remote(Record)]
    pub struct PlaylistEntry {
        pub entry_id: i64,
        pub track: Track,
        /// Its place among the playlist's records.
        pub run: RunRole,
    }

    /// What writing a playlist out to a file managed.
    #[uniffi::remote(Record)]
    pub struct PlaylistExport {
        pub written: u32,
        /// Tracks with no file on this machine — a playlist file is a list of
        /// paths, and an undownloaded remote track has none.
        pub skipped: u32,
    }

    #[uniffi::remote(Record)]
    pub struct ScanSummary {
        pub added: u32,
        pub updated: u32,
        pub removed: u32,
        pub skipped: u32,
        pub errors: Vec<String>,
    }

    #[uniffi::remote(Record)]
    pub struct SyncSummary {
        pub artists: u32,
        pub albums: u32,
        pub tracks: u32,
        /// Non-zero means the run was incomplete and the next one will retry those
        /// albums — worth saying so rather than reporting a clean sync.
        pub albums_failed: u32,
        /// Pages of tracks that could not be fetched. Non-zero means the same.
        pub pages_failed: u32,
        pub favourites_pushed: u32,
        pub favourites_imported: u32,
        pub playlists_pulled: u32,
        pub playlists_pushed: u32,
    }

    /// Which part of a remote sync is running.
    #[uniffi::remote(Enum)]
    pub enum SyncPhase {
        /// Paging through the server's album list. `done` counts albums.
        Albums,
        /// Fetching and writing tracks. `done` counts tracks.
        Tracks,
        /// Recording artist metadata. `total` is the number of artists.
        Artists,
        /// Relinking and tidying up. No counts.
        Finishing,
    }

    /// How far fetching covers ahead has got: records done of those found
    /// without one.
    #[uniffi::remote(Record)]
    pub struct CoverPreload {
        pub done: u64,
        pub total: u64,
    }

    /// How far a remote sync has got. `total` is absent where the server gives no
    /// way to know it in advance.
    #[uniffi::remote(Record)]
    pub struct SyncProgress {
        pub phase: SyncPhase,
        pub done: u64,
        pub total: Option<u64>,
    }

    /// A named pattern from `[organize.patterns]`.
    #[uniffi::remote(Record)]
    pub struct OrganizePattern {
        pub name: String,
        pub pattern: String,
        /// The one `[organize] default` names. Preselected when the sheet opens.
        pub is_default: bool,
    }

    /// What the pattern means for one file.
    #[uniffi::remote(Enum)]
    pub enum PlanOutcome {
        /// Will be moved, or was.
        Move,
        /// Already exactly where the pattern puts it.
        Unchanged,
        /// Something holds the destination. Nothing is ever overwritten, so this
        /// file stays where it is.
        Conflict,
        /// The pattern produced nothing usable, or the move failed.
        Error,
    }

    /// One file's row in the plan: where it is, where the pattern puts it, and
    /// whether that can happen.
    #[uniffi::remote(Record)]
    pub struct OrganizeEntry {
        /// `None` for a file the library holds no row for.
        pub track_id: Option<i64>,
        pub from_path: String,
        /// `None` only when the pattern failed before producing a path at all.
        pub to_path: Option<String>,
        pub outcome: PlanOutcome,
        /// Why this file isn't moving. `None` when it is.
        pub reason: Option<String>,
        /// Names of the cover art, cue sheets and logs travelling with this file.
        /// Named rather than counted: "+1 file" tells you something is coming
        /// along without telling you whether you want it to.
        pub ancillary: Vec<String>,
    }

    /// Every selected file and what happens to it, in plan order.
    ///
    /// Preview and execute answer in the same shape, so the table the user
    /// confirmed is the table that reports what happened.
    #[uniffi::remote(Record)]
    pub struct OrganizePlan {
        pub entries: Vec<OrganizeEntry>,
        pub moved_count: u32,
        pub unchanged_count: u32,
        pub conflict_count: u32,
        pub error_count: u32,
        /// Selected tracks with no local file to move — remote-only, or gone from
        /// disk. Counted so a selection of 20 yielding 12 rows says why.
        pub unresolved: u32,
    }

    /// What importing files from outside the library produced.
    #[uniffi::remote(Record)]
    pub struct ImportSummary {
        /// Library rows for the imported files, in walk order — what the caller
        /// queues.
        pub track_ids: Vec<i64>,
        pub added: u32,
        pub updated: u32,
        pub errors: Vec<String>,
    }

    /// An artist beyond the library: the opening of their Wikipedia article and
    /// whether there is a photograph to ask for.
    #[uniffi::remote(Record)]
    pub struct ArtistInfo {
        pub bio: Option<String>,
        /// The article the biography opens, for reading on and for credit.
        pub bio_url: Option<String>,
        pub has_image: bool,
        /// Photographer and licence.
        pub image_credit: Option<String>,
    }

    /// Sort orders the library browser offers. Applied by the database, because a
    /// listing that is read a page at a time has to be ordered before it is cut.
    #[uniffi::remote(Enum)]
    pub enum AlbumSort {
        /// Newest first. What a library browser should open on — the thing you
        /// just added is the thing you want.
        RecentlyAdded,
        Title,
        Artist,
        /// Newest release first.
        Year,
        /// Oldest release first.
        YearAsc,
        /// Shuffled by the seed passed alongside it. The same seed is the same
        /// order, page after page; a new seed is a new order — which is the point,
        /// it's for turning up records you'd forgotten.
        Random,
        /// Most recently played first. Only with [`BrowseFilter::recent`], which
        /// is what knows when; without it, `RecentlyAdded`.
        LastPlayed,
        /// Fully on this device first, then by how much is: the Downloaded
        /// shelf's order.
        Downloaded,
        /// The closest match to the search first: the search shelf's order.
        /// Without a search, `RecentlyAdded`.
        BestMatch,
    }

    /// Narrowing the album and artist browsers by what the records are. The web
    /// UI's filters, applied by the same queries: an artist passes when any of
    /// their albums does.
    #[uniffi::remote(Record)]
    pub struct BrowseFilter {
        pub favourites: bool,
        /// Only what was played in the last `koan_core::shelves::RECENT_DAYS`:
        /// the Recently Played shelf as a filter.
        pub recent: bool,
        /// Only what can play on this device: the Downloaded shelf as a filter.
        pub downloaded: bool,
        /// Only records in a lossless codec.
        pub lossless: bool,
        /// Only records in this codec, as `BrowseChoices::codecs` names it.
        pub codec: Option<String>,
        /// Release year bounds, inclusive. Records without a date are left out
        /// when either is set.
        pub year_from: Option<i32>,
        pub year_to: Option<i32>,
        pub genre: Option<String>,
    }

    /// What the codec and genre filters offer: the codecs records are in, most
    /// common first, and the genres most records carry.
    #[uniffi::remote(Record)]
    pub struct BrowseChoices {
        pub codecs: Vec<String>,
        pub genres: Vec<String>,
    }

    /// How the track browser orders the library.
    #[uniffi::remote(Enum)]
    pub enum TrackBrowseSort {
        /// Artist, then record, disc and track: the library as it would be shelved.
        Artist,
        Title,
        /// By record title, then in running order.
        Album,
        Duration,
        /// Most recently played first. Only with [`BrowseFilter::recent`]; without
        /// it, by title.
        LastPlayed,
        /// The closest match to the search first: the search shelf's order.
        /// Without a search, as `Artist`.
        BestMatch,
    }

    /// A page of the track browser, and how many tracks pass its filters in all.
    #[uniffi::remote(Record)]
    pub struct TrackListing {
        pub tracks: Vec<Track>,
        pub total: u64,
    }

    #[uniffi::remote(Error)]
    pub enum KoanError {
        Database {
            message: String,
        },
        Audio {
            message: String,
        },
        Player {
            message: String,
        },
        NotFound {
            message: String,
        },
        BadArgument {
            message: String,
        },
        /// The server could not be reached, or gave up part way. Distinct from a
        /// server that answered and said no — this one is worth retrying.
        Remote {
            message: String,
        },
        /// The server answered and will not do it: it lacks the feature, or
        /// refuses this account. Asking again changes nothing.
        Refused {
            message: String,
        },
        /// An import of coefficients that say nothing of their rate. Ask, and
        /// import again with one.
        NeedsSampleRate {
            message: String,
        },
    }

    /// What to offer the output in use about AutoEQ.
    #[uniffi::remote(Enum)]
    pub enum AutoEqOffer {
        /// Its profile: the name says which headphone it is.
        Profile { entry: AutoEqEntry },
        /// A search to pick from: the name says only roughly.
        Search { query: String },
    }

    /// A measurement on a squig.link site.
    #[uniffi::remote(Record)]
    pub struct SquigHit {
        /// Brand, model and variant, as a profile would be named.
        pub name: String,
        pub brand: String,
        pub model: String,
        /// Tips, inserts or a nozzle; empty for the model's own.
        pub variant: String,
        /// The site, as `dspSquigFetch` takes it, and as a person reads it.
        pub site: String,
        pub site_label: String,
        /// The rig, where the site says which.
        pub rig: Option<String>,
        /// In-ear or over-ear, where the site keeps one kind.
        pub in_ear: Option<bool>,
        /// What `dspSquigFetch` takes, with `site`.
        pub file: String,
        /// The credit kept on a correction made from it.
        pub source: String,
        /// Why it cannot be fetched, where its site keeps measurements from apps.
        pub locked: Option<String>,
    }

    /// What a measurement is of: a kind of headphone, or a speaker.
    #[uniffi::remote(Enum)]
    pub enum DspEarKind {
        InEar,
        OverEar,
        Speaker,
    }

    /// What a measurement is read as: `note` to show, and whether it is a
    /// speaker's, corrected to Flat.
    #[uniffi::remote(Record)]
    pub struct DspMeasurementReading {
        pub note: String,
        pub speaker: bool,
    }

    /// A headphone target a correction can be moved to.
    #[uniffi::remote(Record)]
    pub struct DspTargetOption {
        /// What `dspChooseTarget` takes.
        pub id: String,
        pub name: String,
        /// What it does, in a few plain words; empty for one a person added.
        pub does: String,
        /// What it sounds like, in a line; empty for one a person added.
        pub character: String,
    }

    /// What importing a selection of files did: the profiles it made or added
    /// to, and each file it refused, with why.
    #[uniffi::remote(Record)]
    pub struct DspImportSummary {
        pub imported: Vec<String>,
        pub refused: Vec<DspImportRefusal>,
        /// Files imported under another name than their own, and files left
        /// out, each with what happened.
        pub notes: Vec<String>,
        /// The group the presets became, where they became one.
        pub group: Option<String>,
    }

    /// What importing a selection will do, shown before it is done.
    #[uniffi::remote(Record)]
    pub struct DspImportPlan {
        /// A group of presets, one playing at a time, rather than one profile
        /// combined from them all.
        pub group: bool,
        pub files: Vec<String>,
        /// A name to suggest: the group's, from what the files' names share, or
        /// the one profile's.
        pub name: String,
    }

    #[uniffi::remote(Record)]
    pub struct DspImportRefusal {
        pub file: String,
        pub reason: String,
    }

    /// The target a correction was made for, the one chosen, and the others.
    #[uniffi::remote(Record)]
    pub struct DspTargets {
        /// None for a correction built from a measurement, which is made for
        /// `chosen`.
        pub made_for: Option<DspTargetOption>,
        /// Unset when it plays as made.
        pub chosen: Option<String>,
        pub choices: Vec<DspTargetOption>,
    }

    /// A maker in AutoEQ's index.
    #[uniffi::remote(Record)]
    pub struct AutoEqMaker {
        pub name: String,
        /// How many results it has, sources counted apart.
        pub results: u32,
    }

    /// A headphone's result in AutoEQ's index.
    #[uniffi::remote(Record)]
    pub struct AutoEqEntry {
        pub name: String,
        /// Who measured it, and on which rig where they used several: what tells
        /// two results for one headphone apart, and what installing names.
        pub measured_by: String,
        /// What the profile installed from it is called.
        pub profile_name: String,
    }

    /// The DSP profiles, and which the current output plays through.
    #[uniffi::remote(Record)]
    pub struct DspOverview {
        /// Off bypasses every profile.
        pub enabled: bool,
        /// The output device playback goes to, which profiles are chosen by. A
        /// renderer is named by its UDN.
        pub device: Option<String>,
        /// The profile that device plays: its correction.
        pub active: Option<String>,
        /// The tuning it plays on top of its correction.
        pub tuning: Option<String>,
        /// Whether that tuning plays: not on a baked correction, nor where the
        /// chain cannot hold it.
        pub tuning_plays: bool,
        /// What of the output's choices does not play, and why.
        pub left_out: Option<String>,
        /// The EQs of its tuning left out entirely, by name.
        pub left_out_eqs: Vec<String>,
        /// Every device the EQ names, and what each is set to.
        pub outputs: Vec<DspOutputState>,
        /// Every output's tuning, by device: its first EQ.
        pub tunings: std::collections::HashMap<String, String>,
        /// The output's tuning: its EQs in the order they play.
        pub chain: Vec<DspTuningEntry>,
        /// How each EQ of `chain` meets the correction, in the same order.
        pub joins: Vec<DspEqJoin>,
        /// The target the output's correction aims at, by name.
        pub aim: Option<String>,
        /// Whether that target is for in-ears; none where it is not known.
        pub in_ear: Option<bool>,
        /// The preset the output was set from, and whether it was changed since.
        pub preset: Option<String>,
        pub preset_edited: bool,
        pub profiles: Vec<DspProfileSummary>,
        /// What to call the devices named by a UDN, where the renderer is known.
        pub names: std::collections::HashMap<String, String>,
        /// The output plays filters alone: no correction, nothing matched to a
        /// target.
        pub filters_only: bool,
        /// The correction aims at a target and the EQs that play meet it
        /// without conversion, so the chain is drawn as matched.
        #[uniffi(default = false)]
        pub matched: bool,
    }

    /// What a device is set to, for a menu of presets.
    #[uniffi::remote(Record)]
    pub struct DspOutputState {
        pub device: String,
        /// The preset it was set from.
        pub preset: Option<String>,
        /// Changed since it was set from that preset.
        pub preset_edited: bool,
        /// No correction and no tuning: it plays untouched.
        pub flat: bool,
    }

    /// One EQ of an output's tuning.
    #[uniffi::remote(Record)]
    pub struct DspTuningEntry {
        pub name: String,
        /// Switched off, it stays in the tuning and plays nothing.
        pub on: bool,
    }

    /// How an EQ of the tuning meets the correction ahead of it.
    #[uniffi::remote(Record)]
    pub struct DspEqJoin {
        /// The target it was made against, by name.
        pub made_for: Option<String>,
        /// None where nothing is compared: no correction, or one whose target
        /// is not known, or an EQ that does not play.
        pub join: Option<DspJoin>,
        /// What of it does not play as chosen, and why.
        pub note: Option<String>,
        /// For a converted join, the target difference that plays, in dB on
        /// the response grid.
        pub step: Vec<f64>,
        /// For an unknown join, the target it looks made against.
        pub suggestion: Option<DspTargetName>,
    }

    /// A target by id and name.
    #[uniffi::remote(Record)]
    pub struct DspTargetName {
        pub id: String,
        pub name: String,
    }

    /// What a tuning adds on a correction, said to be made against `target`
    /// (none for not saying), in dB on the response grid.
    #[uniffi::remote(Record)]
    pub struct DspMadeAgainstPreview {
        pub target: Option<String>,
        pub db: Vec<f64>,
    }

    #[uniffi::remote(Enum)]
    pub enum DspJoin {
        /// Made against the target the correction aims at.
        Matched,
        /// Made against another: the difference from `from` to `to` plays
        /// first.
        Converted { from: String, to: String },
        /// Made against another, under a correction fitted to a measurement:
        /// the correction is fitted to `to` in place of `from`.
        Refitted { from: String, to: String },
        /// Made against a target not said, so nothing converts.
        Unknown,
    }

    #[uniffi::remote(Record)]
    pub struct DspProfileSummary {
        pub name: String,
        pub devices: Vec<String>,
        pub bands: u32,
        /// Profiles it plays first, for a stack.
        pub layers: u32,
        /// Rates there are impulse responses for.
        pub rates: Vec<u32>,
        /// Why it would not load, if it would not.
        pub problem: Option<String>,
        /// For a group: its members, in order, and the one playing.
        pub members: Vec<String>,
        pub playing: Option<String>,
        pub role: DspRole,
        /// Built from a measurement.
        pub measured: bool,
        /// A preset: a correction and its tuning saved together.
        pub preset: bool,
        /// Changed since it was imported.
        pub edited: bool,
        /// The devices it is chosen for: as their correction, in their tuning,
        /// or as the preset they were set from.
        pub used_on: Vec<String>,
        /// Kept on every device of the account.
        pub everywhere: bool,
        /// The presets and groups that hold it.
        pub held_by: Vec<String>,
        /// Why it cannot move to the other scope, while it cannot.
        pub scope_locked: Option<String>,
        /// The graphic curves among `bands`, and their points in all.
        pub graphics: u32,
        pub points: u32,
        /// The target it was made against, by name.
        pub made_for: Option<String>,
        /// How it would meet the overview device's correction, as `joins` says
        /// of an EQ in that device's tuning.
        pub join: Option<DspJoin>,
    }

    /// What a profile is for. A chain corrects a headphone once.
    #[uniffi::remote(Enum)]
    pub enum DspRole {
        /// Brings a headphone to a target, neutral.
        Correction,
        /// Taste on top of a correction.
        Tuning,
        /// A correction with a tuning already in it.
        Baked,
    }

    /// A curve on `DspResponse.freqs`, in dB.
    #[uniffi::remote(Record)]
    pub struct DspCurve {
        pub db: Vec<f64>,
    }

    /// A layer of a stack, as it plays alone.
    #[uniffi::remote(Record)]
    pub struct DspLayerCurve {
        pub name: String,
        pub on: bool,
        pub db: Vec<f64>,
    }

    /// What a profile does to the sound, computed from the filters the DSP
    /// runs, for drawing.
    #[uniffi::remote(Record)]
    pub struct DspResponse {
        pub freqs: Vec<f64>,
        /// Everything it plays.
        pub total: Vec<f64>,
        /// Each of its own parametric bands alone.
        pub bands: Vec<DspCurve>,
        pub layers: Vec<DspLayerCurve>,
        /// For an AutoEQ correction: the headphone as measured, its target, and
        /// the measurement with everything applied.
        pub measurement: Option<Vec<f64>>,
        pub target: Option<Vec<f64>>,
        pub predicted: Option<Vec<f64>>,
        pub preamp_db: f64,
        /// The flat gain among its filters, which `total` includes; with
        /// `preamp_db`, the level the curve sits at.
        pub gain_db: f64,
        /// For a chain with both: the correction alone, and the tuning on top.
        pub correction: Option<Vec<f64>>,
        pub tuning: Option<Vec<f64>>,
        /// For a split's preview: the baked EQ it comes from.
        pub original: Option<Vec<f64>>,
        /// For an EQ with bands for each channel: the right, where `total` is
        /// the left.
        pub right: Option<Vec<f64>>,
        /// `gain_db` on the right channel.
        pub right_gain_db: f64,
    }

    /// A profile played as part of a stack.
    #[uniffi::remote(Record)]
    pub struct DspLayerInfo {
        pub profile: String,
        pub on: bool,
    }

    /// Everything in one profile, for its page in Settings.
    #[uniffi::remote(Record)]
    pub struct DspProfileDetail {
        pub name: String,
        pub devices: Vec<String>,
        /// The files it was imported from.
        pub source: Vec<String>,
        pub bands: Vec<DspBand>,
        pub impulses: Vec<DspImpulse>,
        /// Gain ahead of the filters at `preamp_rate`, derived unless `preamp_set`.
        pub preamp_db: f64,
        pub preamp_rate: u32,
        pub preamp_set: bool,
        pub problem: Option<String>,
        /// Profiles it plays first, in order.
        pub layers: Vec<DspLayerInfo>,
        /// A group: one of `layers` plays, the one switched on.
        pub group: bool,
        /// Kept on every device of the account through its kōan server, rather
        /// than this one alone.
        pub everywhere: bool,
        /// Where it is kept was chosen, rather than following from what it is.
        pub scope_set: bool,
        /// Why the server would not keep it, while it would not.
        pub sync_problem: Option<String>,
        /// What syncing did to it: a rename, and why.
        pub sync_note: Option<String>,
        pub role: DspRole,
        /// What it is for was said, rather than following from what it is.
        pub role_set: bool,
        /// A chain that corrects twice, made before that was refused: which,
        /// and what to do.
        pub corrects_twice: Option<String>,
        /// The chain in a line each: the headphone it corrects and how, and the
        /// tunings on top.
        pub corrects: Option<String>,
        /// The correction has a tuning baked in.
        pub corrects_baked: bool,
        pub tunings: Vec<String>,
        /// For each of `layers`, what it is for; `None` where it is missing.
        pub layer_roles: Vec<Option<DspRole>>,
        /// Built from a measurement, to `DspTargets.chosen`.
        pub measured: bool,
        /// For a ready-made correction: the target it was made for, by id, if
        /// that is known.
        pub made_for: Option<String>,
        /// For a tuning: the target it was made against, by id, if that is known.
        pub tuned_for: Option<String>,
        /// A preset: a correction and its tuning saved together.
        pub preset: bool,
        /// Changed since it was imported.
        pub edited: bool,
        /// Imported, so it can go back to how it was.
        pub can_revert: bool,
        /// A correction, which plays as made: its bands are not edited.
        pub read_only: bool,
        /// For an EQ of bands alone: one list for both channels, or one each.
        pub channels: Option<DspChannelLayout>,
    }

    /// How an EQ of bands lays them over the two channels.
    #[uniffi::remote(Record)]
    pub struct DspChannelLayout {
        /// A list of bands for each channel, rather than one for both.
        pub stereo: bool,
        /// The two lists say different things.
        pub differ: bool,
    }

    /// Where a new EQ of bands starts.
    #[uniffi::remote(Enum)]
    pub enum DspStarter {
        Flat,
        BassBoost,
        Warm,
        Bright,
    }

    /// One of a profile's filters, in the order they run.
    #[uniffi::remote(Record)]
    pub struct DspBand {
        /// A band — `peaking`, `low_shelf`, `high_shelf`, `low_pass`,
        /// `high_pass`, `notch`, `band_pass`, `all_pass`, `gain`, or one of the
        /// shelves and passes with `_first_order` — or `delay`, `mix` or `graphic`.
        pub kind: String,
        pub freq: f64,
        pub gain_db: f64,
        pub q: f64,
        /// From 0. Empty is every channel.
        pub channels: Vec<u16>,
        /// A delay's length: `delay_ms` and `delay_samples` added together.
        pub delay_ms: f64,
        pub delay_samples: f64,
        /// A mix's outputs, from channel 0.
        pub mix: Vec<DspMixOutput>,
        /// A graphic EQ's points.
        pub curve: Vec<DspPoint>,
    }

    #[uniffi::remote(Record)]
    pub struct DspMixOutput {
        /// `(channel, linear gain)` summed into this output. Empty is silence.
        pub sources: Vec<DspMixSource>,
    }

    #[uniffi::remote(Record)]
    pub struct DspMixSource {
        pub channel: u16,
        pub gain: f64,
    }

    /// A copy saved from an edit: its name, and whether it took the original's
    /// place in the device's tuning.
    #[uniffi::remote(Record)]
    pub struct DspSavedCopy {
        pub name: String,
        pub placed: bool,
    }

    #[uniffi::remote(Record)]
    pub struct DspPoint {
        pub hz: f64,
        pub db: f64,
    }

    /// The ranges a band edited by hand is held to, and the band types that can
    /// be chosen, from `koan_core::audio::dsp::edit`.
    #[uniffi::remote(Record)]
    pub struct DspBandBounds {
        pub hz_min: f64,
        pub hz_max: f64,
        pub db_min: f64,
        pub db_max: f64,
        pub q_min: f64,
        pub q_max: f64,
        /// In the order a menu lists them.
        pub kinds: Vec<DspBandKind>,
    }

    #[uniffi::remote(Record)]
    pub struct DspBandKind {
        /// Its name in the config, as `DspBand::kind` holds it.
        pub id: String,
        /// What a dense table shows, such as "PK".
        pub short: String,
        /// What a menu shows, such as "Peak".
        pub name: String,
        /// A graph draws a handle for it: it has a gain to drag.
        pub draggable: bool,
    }

    /// What an edit naming the filter it last saw did.
    #[uniffi::remote(Enum)]
    pub enum DspEdited {
        /// Saved: the filter as stored, which the next edit of it names, and
        /// whether that changed it.
        Stored { band: DspBand, changed: bool },
        /// Not saved: the filter is no longer the one named, since a filter was
        /// added, removed or changed elsewhere. Read the profile again.
        Changed,
    }

    /// How much of a graphic curve a stroke pulls.
    #[uniffi::remote(Enum)]
    pub enum DspBrush {
        Point,
        Narrow,
        Wide,
    }

    /// The one thing an EQ's page offers on a device.
    #[uniffi::remote(Enum)]
    pub enum DspDeviceAction {
        StopCorrecting,
        MoveToFilters,
        TakeOutOfFilters,
        AddToFilters,
        Correct,
    }

    #[uniffi::remote(Record)]
    pub struct DspImpulse {
        pub file: String,
        pub rate: u32,
        /// `None` for one response applied to every channel.
        pub channels: Option<u32>,
        pub taps: u32,
        pub routes: u32,
        /// Feeds channels into each other, as crossfeed does.
        pub mixes: bool,
        /// Delays channels against each other.
        pub delayed: bool,
        /// Where the response peaks: the delay trimmed from playback.
        pub peak_ms: f64,
    }

    /// How the app is drawn: see `docs/design/koan-theme.md`.
    #[uniffi::remote(Record)]
    pub struct Appearance {
        /// The kōan theme, rather than the platform's look.
        pub koan: bool,
        /// In the kōan theme, icons beside labels.
        pub icons: bool,
        /// The wash and the accent take their colour from the record playing.
        pub record_colours: bool,
        /// On the Mac in the kōan theme, the wash under the whole window.
        pub wash_window: bool,
        /// The accent, wash and indicators in the pride flag's hues, on this device.
        pub rainbow: bool,
        /// Text size on the Mac, in percent.
        pub text_size: u16,
        /// How far the kōan theme holds the wash to its contrast limit, in percent.
        pub wash_tint: u8,
        /// How fast the wash drifts, in percent of its own pace.
        pub wash_drift: u16,
        /// How far the wash turns either way as it drifts, in degrees.
        pub wash_turn: u8,
    }

    /// Everything the settings window reads and writes.
    ///
    /// One record rather than a getter per field: the window shows the whole
    /// configuration at once, and a single read keeps it consistent with itself.
    /// The remote password is deliberately absent — it lives in config.local.toml
    /// and is written through `sign_in_remote`, never read back.
    #[uniffi::remote(Record)]
    pub struct Settings {
        /// Folders and how many tracks each accounts for — a folder is easier to
        /// judge by what it contributed than by its path.
        pub library_folders: Vec<LibraryFolder>,

        pub remote_enabled: bool,
        pub remote_url: String,
        pub remote_username: String,
        /// A password is stored for this server. Not the password.
        pub remote_signed_in: bool,
        /// Tracks the server accounts for.
        pub remote_tracks: u64,
        /// Unix seconds when the library was last known to match the server's.
        /// Read only.
        pub remote_last_sync: Option<i64>,
        pub download_workers: u32,
        /// Human-readable, e.g. "50GB". Empty means unlimited; "0" keeps no
        /// cache beyond the tracks about to play and downloads asked for.
        pub cache_limit: String,
        /// Tracks past the playing one to fetch ahead. `None` fetches the whole
        /// queue; 0 gives up gapless playback.
        pub download_ahead: Option<u32>,
        pub cache_dir: String,
        pub cache_bytes: u64,
        pub auto_sync: bool,
        pub auto_sync_interval_mins: u64,
        /// This device keeps its queue in the account's play queue on the server.
        /// Read only here: turning it on or off goes through `set_server_queue`,
        /// which replaces or saves the queue as it does.
        pub play_queue: bool,
        /// Fetch every record's cover ahead, in the background.
        pub preload_covers: bool,

        /// `off`, `track` or `album`.
        pub replaygain: String,
        pub pre_amp_db: f64,
        pub fade_on_pause: bool,

        /// Open to control from any koan app on the local network.
        pub devices_discoverable: bool,
        /// Devices to reach by address where Bonjour does not: `host:port`.
        pub devices_addresses: Vec<String>,
        /// Addresses whose connections to this device are hung up at once.
        pub devices_refused: Vec<String>,
        /// What devices on the local network may do with this one: `full`
        /// (playback, outputs, presets, volume, hand-off) or `playback` (play and
        /// the queue only).
        pub devices_nearby_control: String,
        /// The Mac app stays in the menu bar with its window closed.
        pub devices_keep_running: bool,

        /// This device serves its library whenever the app is open.
        pub share_enabled: bool,
        pub share_port: u16,
        /// Where invites send phones. Read only here, as `share_url` is:
        /// `set_invite_to` changes both, and Funnel with them.
        pub share_invite_to: koan_runtime::InviteTo,
        /// `share.url`: Funnel's address or the custom one. Empty when there is
        /// none.
        pub share_url: String,
    }

    /// A scanned folder, and what it contributed.
    #[uniffi::remote(Record)]
    pub struct LibraryFolder {
        pub path: String,
        pub tracks: u64,
    }

    /// What a library rebuild removed.
    #[uniffi::remote(Record)]
    pub struct RebuildSummary {
        pub tracks: u64,
        pub albums: u64,
        pub artists: u64,
    }

    /// What clearing the download cache removed.
    #[uniffi::remote(Record)]
    pub struct CacheCleared {
        pub files: u64,
        pub bytes: u64,
    }

    /// The spectrum reduced to three numbers, for indicators that move with the
    /// music without drawing it.
    #[uniffi::remote(Record)]
    pub struct VizLevels {
        pub low: f32,
        pub mid: f32,
        pub high: f32,
    }

    /// See `KoanEngine::art_evictions`.
    #[uniffi::remote(Record)]
    pub struct ArtEvictions {
        /// Pass back as `after` next time.
        pub seq: i64,
        pub albums: Vec<i64>,
        pub artists: Vec<i64>,
        pub tracks: Vec<i64>,
        /// The mark passed was older than the entries kept, or from another
        /// database: everything cached by id goes, not only what is named.
        pub purge: bool,
    }

    /// An account on a server, as one link. What a tapped invite carries and what
    /// an admin sends: `link` for koan, and the password for any other Subsonic
    /// app when the account was just made or given a new one.
    #[uniffi::remote(Record)]
    pub struct Invite {
        pub server: String,
        pub username: String,
        /// What koan trades for an API key; absent from a pasted address with the
        /// account in it.
        pub token: Option<String>,
        pub password: Option<String>,
        pub link: String,
        /// The link for a QR code: its token lapses in minutes where the server
        /// gives one, since anyone who sees the screen can scan it.
        pub qr_link: String,
        pub email_subject: String,
        pub email_text: String,
        pub email_html: String,
        pub mailto: String,
    }

    /// A QR code's modules, `width` to a side, row by row from the top left; a
    /// dark module is `true`. No quiet zone: the drawing adds its own margin.
    #[uniffi::remote(Record)]
    pub struct QrCode {
        pub width: u32,
        pub dark: Vec<bool>,
    }

    /// A pairing this device opened: the code to show, and the link that
    /// approves it.
    #[uniffi::remote(Record)]
    pub struct PairingCode {
        pub id: String,
        /// `XXXX-XXXX`.
        pub code: String,
        pub link: String,
    }

    /// The account's ListenBrainz connection, as its koan server reports it.
    /// The token stays on the server.
    #[uniffi::remote(Record)]
    pub struct ScrobblingConnection {
        /// The ListenBrainz account the plays go to.
        pub account: String,
        /// Plays ListenBrainz has not accepted yet.
        pub pending: i64,
        /// Why ListenBrainz stopped accepting the token, while it does.
        pub error: Option<String>,
    }

    /// A device waiting to be signed in, as the server describes it to an
    /// approver.
    #[uniffi::remote(Record)]
    pub struct PairingInfo {
        pub device: String,
        /// The id the device links with, which its entry among the account's
        /// devices carries once it is signed in. `None` from a server that does
        /// not keep it.
        pub device_id: Option<String>,
        /// The address the request came from, as the server sees it.
        pub from: String,
        /// The address is on a private network (RFC 1918, link-local, unique
        /// local, loopback) rather than the internet.
        pub local: bool,
    }

    /// A pairing link: which server another device is waiting on, and its id.
    #[uniffi::remote(Record)]
    pub struct PairingLink {
        pub server: String,
        pub id: String,
    }

    #[uniffi::remote(Enum)]
    pub enum AccountRole {
        /// Listens.
        Readonly,
        /// Also edits playlists and favourites.
        User,
        /// Also manages the server and its accounts.
        Admin,
    }

    /// Where assistants connect to the signed-in server.
    #[uniffi::remote(Record)]
    pub struct Assistants {
        /// The server's MCP endpoint, to paste into an assistant.
        pub mcp_url: String,
        /// The server's page on connecting one.
        pub connect_url: String,
    }

    /// One of the signed-in account's API keys. Never the key: only its hash is
    /// kept on the server.
    #[uniffi::remote(Record)]
    pub struct ApiKeyInfo {
        pub id: i64,
        pub name: String,
        /// Unix seconds.
        pub created: Option<i64>,
        pub last_used: Option<i64>,
        /// The key this device signs in with.
        pub this_device: bool,
    }

    /// A key just made: the one time it is seen.
    #[uniffi::remote(Record)]
    pub struct NewApiKey {
        pub name: String,
        pub key: String,
    }

    /// An EQ deleted in the last thirty days, which the server keeps a copy of.
    #[uniffi::remote(Record)]
    pub struct DeletedDsp {
        pub uid: String,
        pub name: String,
        /// Whole days until the server forgets it, at least 1 while it is listed.
        pub days_left: u32,
    }

    /// One of the signed-in account's app passwords, for Subsonic apps that sign
    /// in only with a token. Never the password.
    #[uniffi::remote(Record)]
    pub struct AppPasswordInfo {
        pub id: i64,
        pub name: String,
        /// Unix seconds.
        pub created: Option<i64>,
        pub last_used: Option<i64>,
    }

    /// An app password just made: the one time it is seen.
    #[uniffi::remote(Record)]
    pub struct NewAppPassword {
        pub name: String,
        pub password: String,
    }

    #[uniffi::remote(Record)]
    pub struct ServerAccount {
        pub username: String,
        pub role: AccountRole,
    }

    /// How moving the music went.
    #[uniffi::remote(Record)]
    pub struct MoveResult {
        /// Tracks left behind because only the device they were on has them.
        pub left_out: u32,
        /// The destination said it has the music. False when it has not said so
        /// yet: the music was sent and may still arrive (a device asleep takes it
        /// when woken), and stays paused where it was.
        pub started: bool,
        /// The destination is asleep, and takes the music when it wakes.
        pub queued: bool,
        /// The destination would not take it, and why; the music plays on where
        /// it was if it was playing.
        pub error: Option<String>,
    }

    /// A command to another device that did not simply arrive: queued for one
    /// asleep, or not taken. What the app says, since the device's state cannot.
    #[uniffi::remote(Record)]
    pub struct CommandNotice {
        /// Counts up, so the same notice twice is still news.
        pub seq: u64,
        /// The device, by name.
        pub device: String,
        /// Waiting for the device to wake, rather than not taken.
        pub queued: bool,
        /// Reached, and turned down: `detail` is the device's reason.
        pub refused: bool,
        /// Never got there: `detail` says why, naming the device. Otherwise, not
        /// queued or refused, the device took it and failed.
        pub unreached: bool,
        /// Why it was not taken.
        pub detail: String,
    }

    /// How the device being controlled can be reached: see
    /// `koan_core::remote::devices::Reach`.
    #[uniffi::remote(Enum)]
    pub enum DeviceReach {
        Connected,
        Reconnecting,
        Waking,
        Asleep,
        /// Nothing reaches it: commands fail until it is back. `why`, where known.
        OutOfReach {
            why: Option<String>,
        },
    }

    /// Another device koan can play on: one on the same account, or one on the
    /// local network.
    #[uniffi::remote(Record)]
    pub struct DeviceInfo {
        pub id: String,
        pub name: String,
        /// `ios`, `macos` or `linux`.
        pub platform: String,
        /// Signed in to the same account: reachable from anywhere, through the
        /// server.
        pub account: bool,
        /// The account that owns it, for a device shared with this one.
        pub owner: Option<String>,
        /// Found on the local network.
        pub nearby: bool,
        /// Reachable at once. False for a phone iOS has suspended, which a
        /// command wakes and music reaches as a notification to tap.
        pub awake: bool,
        /// Not heard from for longer than a heartbeat.
        pub asleep: bool,
        /// Can be woken from asleep. An asleep device that cannot is shown and
        /// cannot be chosen.
        pub wakeable: bool,
        /// An Apple TV of the account's that is set up to be woken: see
        /// `begin_tv_pairing`.
        pub tv_paired: bool,
        /// An Apple TV of the account's that this device can set up to be woken.
        pub tv_pairable: bool,
        /// Unix seconds when it was last reachable.
        pub last_seen: Option<i64>,
        /// While it is being woken, the stage: `network`, `push` or
        /// `notification`.
        pub waking: Option<String>,
        /// Why the last attempt to wake it failed.
        pub wake_failed: Option<String>,
        /// Plays from the same library, so music can be handed between the two.
        pub same_library: bool,
        pub state: PlayState,
        pub title: Option<String>,
        pub artist: Option<String>,
        /// `artist` with each artist it credits linked, where this library has
        /// the track; empty when nothing is playing.
        pub artist_credit: Vec<CreditPart>,
        pub album: Option<String>,
        /// This library's row for what it is playing, when it has one: for its
        /// artwork and its heart.
        pub track_id: Option<i64>,
        pub album_id: Option<i64>,
        /// Where the playhead is as this is published, run on from the report
        /// while it is playing. The client runs it on from arrival in turn; a raw
        /// report would rewind each time the list is published again for some
        /// other device's change.
        pub position_ms: u64,
        pub duration_ms: u64,
        /// Why it cannot be reached, for a device found but not connected or
        /// the one being controlled while it is out of reach.
        pub problem: Option<String>,
    }

    /// A UPnP renderer on the network: an amplifier or streamer that plays a
    /// file it is handed a URL to.
    #[uniffi::remote(Record)]
    pub struct RendererInfo {
        pub udn: String,
        pub name: String,
        pub manufacturer: String,
        pub model: String,
        /// Takes the next track before this one ends, so albums play without gaps.
        pub gapless: bool,
        /// Playing or paused when last asked, by whatever drives it. Picking it
        /// takes it over.
        pub busy: bool,
    }

    /// What the device in view plays through: this one's outputs, or those of the
    /// device it controls, as that device published them.
    #[uniffi::remote(Record)]
    pub struct OutputsInfo {
        /// The device they belong to, by name, when it is another one.
        pub owner: Option<String>,
        /// Its own audio devices: on a phone, the route the system chose.
        pub devices: Vec<OutputInfo>,
        pub renderers: Vec<OutputInfo>,
        pub current: OutputChoice,
        /// The volume of the renderer it plays to, when it has one.
        pub volume: Option<u8>,
        /// Its EQ presets, and whether processing is on there (always, from
        /// apps that make a device flat instead).
        pub profiles: Vec<String>,
        pub dsp_enabled: bool,
    }

    /// One output: an audio device by name, or a renderer by UDN.
    #[uniffi::remote(Record)]
    pub struct OutputInfo {
        pub id: String,
        pub name: String,
        /// How it is connected: `usb`, `bluetooth`, `upnp` and the rest.
        pub kind: String,
        /// A renderer's make and model.
        pub detail: String,
        /// Playing or paused for something else.
        pub busy: bool,
        /// The EQ preset it was set from.
        pub preset: Option<String>,
        /// Its EQ is no preset as saved; with no preset and not this, it is flat.
        pub unsaved: bool,
    }

    /// An output to play through.
    #[uniffi::remote(Enum)]
    pub enum OutputChoice {
        /// The system's default output.
        Default,
        Device {
            name: String,
        },
        Renderer {
            udn: String,
        },
    }

    /// The renderer this koan is playing to in place of its own output.
    #[uniffi::remote(Record)]
    pub struct RendererOutput {
        pub udn: String,
        pub name: String,
        /// 0–100, when the renderer has a volume control.
        pub volume: Option<u8>,
        /// Why the last track was skipped there, until one plays.
        pub problem: Option<String>,
    }

    /// What the server this app signs in to turned out to be, and what it and
    /// this device offer. Settings shows it.
    #[uniffi::remote(Record)]
    pub struct ConnectionInfo {
        /// The server this device signs in to, while it has an account there.
        pub signed_in_to: Option<String>,
        /// OpenSubsonic's `type`: `koan`, `navidrome`. `None` from an older
        /// server, or before one has answered.
        pub server_kind: Option<String>,
        pub server_version: Option<String>,
        pub open_subsonic: bool,
        /// Each extension and the versions the server offers, e.g. `koanLink 1`.
        pub extensions: Vec<ServerExtension>,
        /// The server can pass commands between this account's devices.
        pub devices: bool,
        /// The link to the server is up.
        pub linked: bool,
        /// The port this device listens on for others on the network, while it
        /// is discoverable.
        pub listening_port: Option<u16>,
        /// iOS has not let this app onto the local network, so nothing there can
        /// be found or reached until the person allows it in Settings.
        pub local_network_blocked: bool,
        /// This device, as others see it.
        pub this_device: String,
        /// The server lets this device be shared with other accounts on it.
        pub sharing: bool,
        /// The accounts this device is shared with.
        pub shared_with: Vec<String>,
        /// Why the server refused the last change to them.
        pub share_error: Option<String>,
        /// The server's other accounts, to share with.
        pub share_accounts: Vec<String>,
        /// The library is narrowed to what can play here: turned on by hand, or
        /// the server out of reach.
        pub offline: bool,
        /// Turned on by hand, rather than by the server being out of reach.
        pub offline_manual: bool,
        /// The server can sign a device without a keyboard in, once someone here
        /// approves it.
        pub pairing: bool,
        /// The account's scrobbling can be set up from here
        /// (`koanScrobbling`).
        pub scrobbling: bool,
        /// The server refused the stored credential (a revoked API key, a changed
        /// password) when it was last used, and has not accepted it since.
        pub sign_in_refused: bool,
        /// Whether this device can use its server, for the banner the apps show
        /// on every page while it cannot.
        pub standing: ServerStanding,
        /// The servers devices on this network are signed in to, announced over
        /// Bonjour: what a device not signed in yet offers to sign in to.
        pub nearby_servers: Vec<NearbyServer>,
        /// The last command to another device that did not simply arrive.
        pub command_notice: Option<CommandNotice>,
        /// Who is connected to this device and whom it is connected to: see
        /// `koan_core::remote::connections`.
        pub connections: Vec<ConnectedInfo>,
        /// Devices disconnected from Settings, held off until allowed back.
        pub held: Vec<HeldInfo>,
    }

    /// See `koan_core::helpers::ServerStanding`.
    #[uniffi::remote(Enum)]
    pub enum ServerStanding {
        Fine,
        SignedOut,
        Refused,
        Unreachable,
    }

    /// A device disconnected from Settings and held off.
    #[uniffi::remote(Record)]
    pub struct HeldInfo {
        /// What `allow_held` takes.
        pub addr: String,
        /// Its name, when it proved who it is.
        pub name: Option<String>,
    }

    /// A connection to this device or from it.
    #[uniffi::remote(Record)]
    pub struct ConnectedInfo {
        /// What `end_connection` takes: only a connection on the network to this
        /// device has one.
        pub key: Option<u64>,
        /// Through the server, rather than on the local network.
        pub via_server: bool,
        /// The other end can control this device.
        pub inbound: bool,
        /// The device's name, or the server's or a stranger's address.
        pub name: String,
        /// The address on the network, which `devices.refused` takes.
        pub addr: Option<String>,
        /// It proved it is one of the account's devices.
        pub own: bool,
        /// It proved it is a device another account shares: that account.
        pub owner: Option<String>,
        /// Unix seconds. `None` where this device cannot know.
        pub since: Option<i64>,
    }

    /// A server another device on this network announced it is signed in to.
    #[uniffi::remote(Record)]
    pub struct NearbyServer {
        pub url: String,
        /// The devices that announced it, by name.
        pub devices: Vec<String>,
    }

    #[uniffi::remote(Record)]
    pub struct ServerExtension {
        pub name: String,
        pub versions: Vec<i64>,
    }

    /// What a command acts on, named rather than resolved: the engine reads an
    /// album's, an artist's or a playlist's tracks on the command lane, in turn
    /// with every other command, so a client never waits on that read before its
    /// command takes its place.
    #[uniffi::remote(Enum)]
    pub enum TrackSet {
        Tracks { ids: Vec<i64> },
        Album { id: i64 },
        Artist { id: i64 },
        Playlist { id: i64 },
    }

    /// Where in the queue tracks go. `Next` and `AppendAndPlay` are read against
    /// the queue as the player applies them, never against a client's copy.
    #[uniffi::remote(Enum)]
    pub enum Placement {
        /// Replace the queue and play from `start_at` (the first track by
        /// default), or `shuffled` into a random order from its top, with shuffle
        /// play and repeat off.
        Replace {
            start_at: Option<u32>,
            shuffled: bool,
        },
        /// Append; playback starts if the player was stopped.
        Append,
        /// Append and play the first of them.
        AppendAndPlay,
        /// After the playing track, or appended when nothing is playing.
        Next,
        /// After this queue item.
        After { queue_item_id: String },
    }
}
#[allow(unused_imports)]
pub use types::*;
