//! iOS: the audio session the engine plays through, and the engine told when
//! the app leaves the screen and comes back, as the SwiftUI app's
//! `AudioSession` and scene phases do. The session's category is playback,
//! so music plays on with the screen locked and the silent switch on; the
//! app's Info.plist asks for the audio background mode.

use std::sync::Arc;

use koan_kit::app::App;
use koan_kit::app::platform::AppLifecyclePhase;
use koan_runtime::{AudioSessionHost, KoanEngine};
use objc2_avf_audio::{
    AVAudioSession, AVAudioSessionCategoryPlayback, AVAudioSessionSetActiveOptions,
};

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

pub fn init(engine: &Arc<KoanEngine>, cx: &mut App) {
    // SAFETY: the shared session, on the main thread at launch.
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
    engine.set_audio_session(Arc::new(Session));
    let engine = engine.clone();
    cx.on_app_lifecycle(move |phase, _| match phase {
        AppLifecyclePhase::Background => engine.set_background(true),
        AppLifecyclePhase::Foreground | AppLifecyclePhase::Active => engine.set_background(false),
        _ => {}
    })
    .detach();
}
