//! iOS: the audio session the engine plays through, and the engine told when
//! the app leaves the screen and comes back, as the SwiftUI app's
//! `AudioSession` and scene phases do. The session's category is playback,
//! so music plays on with the screen locked and the silent switch on; the
//! app's Info.plist asks for the audio background mode. A call, Siri or
//! another app taking the audio pauses the music and, when iOS says to,
//! resumes it; headphones unplugged pause it.

use std::ptr::NonNull;
use std::sync::Arc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use futures_channel::mpsc;
use futures_util::StreamExt as _;
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNumber, NSOperationQueue, NSString,
};

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
            (number(note, AVAudioSessionRouteChangeReasonKey) == Some(OLD_DEVICE_UNAVAILABLE))
                .then_some(Heard::RouteGone)
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
            }
        }
    })
    .detach();
}

pub fn init(engine: &Arc<KoanEngine>, cx: &mut App) {
    configure();
    engine.set_audio_session(Arc::new(Session));
    follow(engine.clone(), cx);
    let engine = engine.clone();
    cx.on_app_lifecycle(move |phase, _| match phase {
        AppLifecyclePhase::Background => engine.set_background(true),
        AppLifecyclePhase::Foreground | AppLifecyclePhase::Active => engine.set_background(false),
        _ => {}
    })
    .detach();
}
