//! iOS: the audio session the engine plays through, and the engine told when
//! the app leaves the screen and comes back, as the SwiftUI app's
//! `AudioSession` and scene phases do. The session's category is playback,
//! so music plays on with the screen locked and the silent switch on; the
//! app's Info.plist asks for the audio background mode. A call, Siri or
//! another app taking the audio pauses the music and, when iOS says to,
//! resumes it; headphones unplugged pause it.

use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use block2::RcBlock;
use futures_channel::mpsc;
use futures_util::StreamExt as _;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_avf_audio::AVAudioSessionPortDescription;
use objc2_foundation::{
    NSFileManager, NSNotification, NSNotificationCenter, NSNumber, NSOperationQueue, NSString,
};
use objc2_ui_kit::{UIApplicationDelegate, UIDevice};

use koan_kit::app::App;
use koan_kit::app::platform::AppLifecyclePhase;
use koan_runtime::{AudioSessionHost, KoanEngine};
use objc2_avf_audio::{
    AVAudioSession, AVAudioSessionCategoryPlayback, AVAudioSessionInterruptionNotification,
    AVAudioSessionInterruptionOptionKey, AVAudioSessionInterruptionReasonKey,
    AVAudioSessionInterruptionTypeKey, AVAudioSessionMediaServicesWereResetNotification,
    AVAudioSessionRouteChangeNotification, AVAudioSessionRouteChangeReasonKey,
    AVAudioSessionSetActiveOptions,
};

use crate::mirror::Mirror;

/// The shared `AVAudioSession`, activated at the rate a track wants the
/// moment before it plays and released once output has stopped a while.
struct Session;

impl AudioSessionHost for Session {
    fn activate(&self, sample_rate: f64) -> Option<f64> {
        // SAFETY: the shared session is safe to use from any thread.
        unsafe {
            let session = AVAudioSession::sharedInstance();
            prefer(&session, sample_rate);
            match session.setActive_error(true) {
                Ok(()) => Some(session.sampleRate()),
                Err(e) => {
                    log::warn!("audio session refused activation: {e:?}");
                    None
                }
            }
        }
    }

    fn follow(&self, sample_rate: f64) -> f64 {
        // SAFETY: as above.
        unsafe {
            let session = AVAudioSession::sharedInstance();
            prefer(&session, sample_rate);
            session.sampleRate()
        }
    }

    fn release(&self) {
        // SAFETY: as above.
        let released = unsafe {
            AVAudioSession::sharedInstance().setActive_withOptions_error(
                false,
                AVAudioSessionSetActiveOptions::NotifyOthersOnDeactivation,
            )
        };
        if let Err(e) = released {
            log::warn!("audio session refused deactivation: {e:?}");
        }
    }
}

/// Asks for `sample_rate`, unless it is asked for already: asking again on a
/// route change could itself move the route.
///
/// # Safety
/// `session` is the shared session.
unsafe fn prefer(session: &AVAudioSession, sample_rate: f64) {
    // SAFETY: per the function's contract.
    unsafe {
        if session.preferredSampleRate() == sample_rate {
            return;
        }
        if let Err(e) = session.setPreferredSampleRate_error(sample_rate) {
            log::warn!("audio session refused {sample_rate} Hz: {e:?}");
        }
    }
}

/// Sets the session's category and buffer, as at launch and after iOS
/// resets its media services.
fn configure() {
    // SAFETY: the shared session, on the main thread.
    unsafe {
        let session = AVAudioSession::sharedInstance();
        if let Some(playback) = AVAudioSessionCategoryPlayback
            && let Err(e) = session.setCategory_error(playback)
        {
            log::warn!("audio session refused the playback category: {e:?}");
        }
        if let Err(e) = session.setPreferredIOBufferDuration_error(0.093) {
            log::warn!("audio session refused its buffer duration: {e:?}");
        }
    }
}

/// What the session told the app, read out of its notifications.
enum Heard {
    /// Something took the audio. `route_gone` when it was a route going away
    /// (a voice assistant handing the speaker back), which iOS ends with no
    /// "ended".
    Began { route_gone: bool },
    /// The interruption is over; `resume` when iOS says to resume.
    Ended { resume: bool },
    /// The route in use went away: headphones unplugged.
    RouteGone,
    /// iOS reset its media services, and every audio object with them.
    Reset,
    /// The port audio goes to may have changed.
    Route,
}

/// `AVAudioSessionInterruptionType.began`.
const BEGAN: usize = 1;
/// `AVAudioSessionInterruptionOptions.shouldResume`.
const SHOULD_RESUME: usize = 1;
/// `AVAudioSessionInterruptionReason.routeDisconnected`.
const ROUTE_DISCONNECTED: usize = 4;
/// `AVAudioSessionRouteChangeReason.oldDeviceUnavailable`.
const OLD_DEVICE_UNAVAILABLE: usize = 2;

/// A number from a notification's user info.
fn number(note: &NSNotification, key: Option<&'static NSString>) -> Option<usize> {
    let info = note.userInfo()?;
    let value = info.objectForKey(key?)?;
    let value = value.downcast::<NSNumber>().ok()?;
    Some(value.unsignedIntegerValue())
}

/// Hears the session's notifications on the main queue, for `follow`.
fn listen() -> mpsc::UnboundedReceiver<Heard> {
    let (heard, hearing) = mpsc::unbounded();
    // SAFETY: the notifications are posted to the main queue, and the
    // centre keeps the observers for the app's life.
    unsafe {
        let centre = NSNotificationCenter::defaultCenter();
        let queue = NSOperationQueue::mainQueue();
        let observe = |name: Option<&'static NSString>,
                       read: fn(&NSNotification) -> Option<Heard>| {
            let heard = heard.clone();
            let block = RcBlock::new(move |note: NonNull<NSNotification>| {
                if let Some(event) = read(note.as_ref()) {
                    heard.unbounded_send(event).ok();
                }
            });
            let observer =
                centre.addObserverForName_object_queue_usingBlock(name, None, Some(&queue), &block);
            std::mem::forget(observer);
        };
        observe(AVAudioSessionInterruptionNotification, |note| {
            let kind = number(note, AVAudioSessionInterruptionTypeKey)?;
            Some(if kind == BEGAN {
                Heard::Began {
                    route_gone: number(note, AVAudioSessionInterruptionReasonKey)
                        == Some(ROUTE_DISCONNECTED),
                }
            } else {
                Heard::Ended {
                    resume: number(note, AVAudioSessionInterruptionOptionKey)
                        .is_some_and(|options| options & SHOULD_RESUME != 0),
                }
            })
        });
        observe(AVAudioSessionRouteChangeNotification, |note| {
            Some(
                if number(note, AVAudioSessionRouteChangeReasonKey) == Some(OLD_DEVICE_UNAVAILABLE)
                {
                    Heard::RouteGone
                } else {
                    Heard::Route
                },
            )
        });
        observe(AVAudioSessionMediaServicesWereResetNotification, |_| {
            Some(Heard::Reset)
        });
    }
    hearing
}

/// Acts on what the session says, as the SwiftUI app does. The phone's own
/// audio events are about the phone: while another device is controlled,
/// or a renderer plays, they change nothing here.
fn follow(engine: Arc<KoanEngine>, cx: &mut App) {
    let mut hearing = listen();
    cx.spawn(async move |cx: &mut koan_kit::app::AsyncApp| {
        // Playing when the interruption began: what its end resumes. iOS can
        // send several "began" for one, and the later find it paused.
        let mut was_playing = false;
        let mut route_lost_at: Option<Instant> = None;
        // What arrived while a route-gone interruption waited to settle.
        let mut pending: Option<Heard> = None;
        loop {
            let heard = match pending.take() {
                Some(heard) => heard,
                None => match hearing.next().await {
                    Some(heard) => heard,
                    None => break,
                },
            };
            let (here, renderer) = cx.update(|cx| {
                let mirror = cx.global::<Mirror>();
                let renderer = mirror.renderers.read(cx).0.output.is_some();
                let controlled = mirror.devices.read(cx).0.target.is_some();
                (!controlled && !renderer, renderer)
            });
            let end = |resume: bool, was_playing: &mut bool| {
                let resume = resume && *was_playing && here;
                *was_playing = false;
                // The unit iOS stopped does not start again: it is made
                // afresh, unless a renderer is the output.
                if !renderer {
                    let engine = engine.clone();
                    std::thread::spawn(move || {
                        futures::executor::block_on(async {
                            engine.clone().restart_output().await.ok();
                            if resume {
                                engine.resume().await.ok();
                            }
                        })
                    });
                }
            };
            match heard {
                Heard::Began { route_gone } => {
                    if here {
                        was_playing |= engine.is_playing();
                        let engine = engine.clone();
                        std::thread::spawn(move || futures::executor::block_on(engine.pause()));
                    }
                    // Taken back once the route has settled, as iOS sends
                    // no "ended" for it; music stays paused if the route
                    // itself went (headphones pulled out).
                    // What arrives within the second decides instead.
                    if route_gone {
                        let settled = cx.background_executor().timer(Duration::from_secs(1));
                        match futures::future::select(Box::pin(settled), hearing.next()).await {
                            futures::future::Either::Left(_) => {
                                let lost = route_lost_at
                                    .is_some_and(|at| at.elapsed() < Duration::from_secs(3));
                                end(!lost, &mut was_playing);
                            }
                            futures::future::Either::Right((next, _)) => pending = next,
                        }
                    }
                }
                Heard::Ended { resume } => {
                    end(resume, &mut was_playing);
                }
                Heard::RouteGone => {
                    report_route(&engine);
                    route_lost_at = Some(Instant::now());
                    if here {
                        let engine = engine.clone();
                        std::thread::spawn(move || futures::executor::block_on(engine.pause()));
                    }
                }
                Heard::Reset => {
                    configure();
                    end(false, &mut was_playing);
                }
                Heard::Route => report_route(&engine),
            }
        }
    })
    .detach();
}

/// Tells the engine the name of the port audio now goes to ("AirPods Pro",
/// "Speaker"), which DSP profiles are chosen by.
fn report_route(engine: &Arc<KoanEngine>) {
    // SAFETY: the shared session is safe to use from any thread.
    let name = unsafe {
        let outputs = AVAudioSession::sharedInstance().currentRoute().outputs();
        outputs
            .firstObject()
            .map(|port: Retained<AVAudioSessionPortDescription>| port.portName().to_string())
    };
    if let Some(name) = name {
        let engine = engine.clone();
        std::thread::spawn(move || {
            futures::executor::block_on(engine.set_audio_route(name)).ok();
        });
    }
}

/// What the system calls this phone, for the account's other devices.
pub fn device_name() -> Option<String> {
    let main = MainThreadMarker::new()?;
    Some(UIDevice::currentDevice(main).name().to_string())
}

/// What the share extension opens after leaving something in the app group.
pub const SHARE_INBOX_LINK: &str = "koan://dsp-inbox";

/// The app group the share extension leaves its files in.
const SHARE_GROUP: &str = "group.cc.blit.koan";

/// Imports EQ and filter files iOS opened in kōan.
pub fn import_opened(paths: Vec<PathBuf>, cx: &mut App) {
    let dsp = crate::dsp::model(cx);
    dsp.update(cx, |dsp, cx| dsp.import_files(paths, None, None, None, cx));
}

/// What the share extension left in the app group, one folder per share:
/// each moved to a temporary directory first, so it is taken exactly once,
/// and an import that stops to ask about it still has its files.
pub fn collect_shared(cx: &mut App) {
    let dsp = crate::dsp::model(cx);
    cx.spawn(async move |cx: &mut koan_kit::app::AsyncApp| {
        let shares = cx
            .background_executor()
            .spawn(async { take_shares(&group_inbox()?) })
            .await
            .unwrap_or_default();
        for files in shares {
            dsp.update(cx, |dsp, cx| dsp.import_files(files, None, None, None, cx));
        }
    })
    .detach();
}

fn group_inbox() -> Option<PathBuf> {
    let group = NSString::from_str(SHARE_GROUP);
    let container = NSFileManager::defaultManager()
        .containerURLForSecurityApplicationGroupIdentifier(&group)?;
    Some(PathBuf::from(container.path()?.to_string()).join("Inbox"))
}

fn take_shares(inbox: &Path) -> Option<Vec<Vec<PathBuf>>> {
    let mut taken = Vec::new();
    for (n, share) in std::fs::read_dir(inbox).ok()?.flatten().enumerate() {
        let to = std::env::temp_dir().join(format!("shared-{}-{n}", std::process::id()));
        if std::fs::rename(share.path(), &to).is_err() {
            continue;
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&to)
            .into_iter()
            .flatten()
            .flatten()
            .map(|file| file.path())
            .collect();
        files.sort();
        if files.is_empty() {
            let _ = std::fs::remove_dir_all(&to);
        } else {
            taken.push(files);
        }
    }
    Some(taken)
}

/// What the app's Swift asked to be told. Set before the app runs.
#[derive(Clone, Copy)]
struct Told {
    /// The engine is open: `opened_engine` answers.
    engine: extern "C" fn(),
    /// The server signed in to can send notifications.
    alerts: extern "C" fn(),
}

static TOLD: OnceLock<Told> = OnceLock::new();

/// The app's entry point, from its Swift `main`. Does not return.
#[unsafe(no_mangle)]
pub extern "C" fn koan_gui_main() {
    crate::launch::run();
}

/// Sends `delegate` the application delegate messages GPUI does not handle
/// itself: push registration, and the background pushes that wake the app.
///
/// # Safety
/// `delegate` is an object implementing `UIApplicationDelegate`, passed on
/// the main thread before `koan_gui_main`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn koan_gui_forward_delegate(delegate: *mut AnyObject) {
    // SAFETY: per the function's contract.
    let delegate = unsafe { Retained::retain(delegate) }.expect("a delegate");
    // SAFETY: per the function's contract, it implements the protocol.
    let delegate: Retained<ProtocolObject<dyn UIApplicationDelegate>> =
        unsafe { Retained::cast_unchecked(delegate) };
    gpui_ios::ios::forward_application_delegate(&delegate);
}

/// The callbacks the app's Swift is told through, set before `koan_gui_main`.
#[unsafe(no_mangle)]
pub extern "C" fn koan_gui_on_events(engine: extern "C" fn(), alerts: extern "C" fn()) {
    let _ = TOLD.set(Told { engine, alerts });
}

/// The name the server lists when it can send notifications.
const PUSH_EXTENSION: &str = "koanPush";

/// Asks the Swift side to request permission for notifications whenever the
/// app is in front and the signed-in server can send them.
fn ask_for_alerts(front: bool, cx: &mut App) {
    if !front {
        return;
    }
    let connection = cx.global::<Mirror>().connection.read(cx);
    let offered = connection
        .0
        .as_ref()
        .is_some_and(|c| c.extensions.iter().any(|e| e.name == PUSH_EXTENSION));
    if offered && let Some(told) = TOLD.get() {
        (told.alerts)();
    }
}

pub fn init(engine: &Arc<KoanEngine>, cx: &mut App) {
    configure();
    engine.set_audio_session(Arc::new(Session));
    report_route(engine);
    follow(engine.clone(), cx);
    koan_ffi::adopt_engine(engine.clone());
    if let Some(told) = TOLD.get() {
        (told.engine)();
    }
    let engine = engine.clone();
    cx.on_app_lifecycle(move |phase, cx| match phase {
        AppLifecyclePhase::Background => engine.set_background(true),
        AppLifecyclePhase::Foreground | AppLifecyclePhase::Active => {
            engine.set_background(false);
            // Suspended in the background, the link to the server went with
            // the rest of the app.
            engine.link_nudge();
            if cx.has_global::<Mirror>() {
                collect_shared(cx);
                ask_for_alerts(true, cx);
            }
        }
        _ => {}
    })
    .detach();
}

/// Tells the engine whether it is playing, which decides what runs in the
/// background, and asks for notifications once a server offers them.
///
/// Called once the app's globals are installed.
pub fn started(engine: Arc<KoanEngine>, cx: &mut App) {
    collect_shared(cx);
    let mirror = cx.global::<Mirror>();
    let (playback, connection) = (mirror.playback.clone(), mirror.connection.clone());
    let mut was_playing = None;
    cx.observe(&playback, move |playback, cx| {
        let playing = matches!(playback.read(cx).0.state, koan_runtime::PlayState::Playing);
        if was_playing != Some(playing) {
            was_playing = Some(playing);
            engine.set_playing(playing);
        }
    })
    .detach();
    cx.observe(&connection, |_, cx| ask_for_alerts(true, cx))
        .detach();
}
