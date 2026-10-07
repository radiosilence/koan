use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::backend::{AudioBackend, AudioEngineHandle, BackendError, DeviceInfo, SampleRateWatch};
use super::engine;

/// iOS output, through RemoteIO.
///
/// The engine is the same one macOS uses — see `engine.rs`. What differs is
/// everything around it, and mostly by subtraction: iOS has no device list, no
/// nominal sample rate to set, and no exclusive access. `AudioHardware.h` is
/// not in the SDK at all, so this is not a matter of writing the FFI.
///
/// What stands in for a device is the audio session's current route, which the
/// host app owns: it decides the category and handles interruptions and route
/// changes. That has to live where there is a run loop and an app lifecycle, so
/// it is Swift's. The engine reaches it only through `AudioSession`.
///
/// The device rate is the session's. Each track's rate is asked for as the
/// session's preferred rate, and the rate the hardware then runs at is read
/// back. A USB DAC that supports the source rate is switched to it, and
/// nothing resamples; the built-in speaker, Bluetooth and AirPlay stay at
/// their own rate, RemoteIO resamples, and the rate read back says so.
pub struct IosAudioBackend;

/// The name of the port the session is routed to, as the app last heard it:
/// "AirPods Pro", "Headphones", "Speaker". What DSP profiles are keyed on.
static ROUTE: parking_lot::RwLock<Option<String>> = parking_lot::RwLock::new(None);

/// Told by the app on activation and each route change. A new route is a new
/// output to every device controlling this one, so they are told.
pub fn set_route(name: String) {
    let changed = ROUTE.write().replace(name.clone()).as_ref() != Some(&name);
    if changed {
        crate::remote::outputs::refresh_devices();
    }
}

/// The app's audio session, as output needs it.
///
/// koan takes the session exclusively when it plays, and only then: an app
/// that activates a `.playback` session on launch or on coming to the front
/// stops whatever else the phone was playing, with nothing pressed. So the
/// engine asks for the session the moment before its output unit starts, and
/// the player lets it go once output has been stopped for a while.
///
/// Both are called on the player thread, in order, which is what keeps a
/// release from landing between an activation and the start it was for.
pub trait AudioSession: Send + Sync {
    /// Make the session active for playback, asking for `sample_rate` as its
    /// preferred rate, and answer the rate the hardware runs at. Blocks until
    /// the session is active; `None` if the system refused, as it does during
    /// a call or to an app in the background with nothing to interrupt for.
    fn activate(&self, sample_rate: f64) -> Option<f64>;
    /// Nothing has played for a while: deactivate, and tell the other apps
    /// they may play again.
    fn release(&self);
}

static SESSION: parking_lot::RwLock<Option<Arc<dyn AudioSession>>> = parking_lot::RwLock::new(None);

/// The session has been activated and not since released.
static HELD: AtomicBool = AtomicBool::new(false);

/// Told once by the app, before anything can play.
pub fn set_session(session: Arc<dyn AudioSession>) {
    *SESSION.write() = Some(session);
}

/// The rate the session last said the hardware runs at, as `f64` bits.
static GRANTED: AtomicU64 = AtomicU64::new(0);

/// The rate last asked for, as `f64` bits: the current output's, which a new
/// route is asked for again.
static REQUESTED: AtomicU64 = AtomicU64::new(0);

type RateWatch = Arc<dyn Fn(f64) + Send + Sync>;

/// Told the granted rate on each activation. One at a time: the player's
/// current session's.
static WATCH: parking_lot::Mutex<Option<RateWatch>> = parking_lot::Mutex::new(None);

fn activate(session: &dyn AudioSession, sample_rate: f64) -> Option<f64> {
    REQUESTED.store(sample_rate.to_bits(), Ordering::Release);
    let granted = session.activate(sample_rate)?;
    HELD.store(true, Ordering::Release);
    GRANTED.store(granted.to_bits(), Ordering::Release);
    Some(granted)
}

/// Called by the engine just before its output unit starts. False if the
/// session could not be activated, when starting would only play silence.
/// A session taken back after a release may come back at another rate than
/// the one the player last heard, so the granted rate goes to the watch.
pub(crate) fn activate_session(sample_rate: f64) -> bool {
    let session = SESSION.read().clone();
    let Some(session) = session else { return true };
    let Some(granted) = activate(session.as_ref(), sample_rate) else {
        return false;
    };
    tell_watch(granted);
    true
}

fn tell_watch(granted: f64) {
    let watch = WATCH.lock().clone();
    if let Some(watch) = watch {
        watch(granted);
    }
}

/// The route changed under a held session: a DAC plugged in or pulled out,
/// or the hardware settling at the rate it was asked for. The new route is
/// asked for the output's rate, and the player told what it runs at. A
/// session not held is asked when the engine next starts.
pub(crate) fn follow_route() {
    if !session_held() {
        return;
    }
    let session = SESSION.read().clone();
    let Some(session) = session else { return };
    let rate = f64::from_bits(REQUESTED.load(Ordering::Acquire));
    match activate(session.as_ref(), rate) {
        Some(granted) => tell_watch(granted),
        None => log::warn!("audio session refused activation on a new route"),
    }
}

pub(crate) fn session_held() -> bool {
    HELD.load(Ordering::Acquire)
}

pub(crate) fn release_session() {
    let session = SESSION.read().clone();
    if let Some(session) = session {
        HELD.store(false, Ordering::Release);
        session.release();
    }
}

/// The one device there is: whatever the session is routed to.
///
/// Named rather than enumerated, because the name is all iOS will tell us
/// without going through the session — and the session is the app's.
fn current_route() -> DeviceInfo {
    DeviceInfo {
        name: ROUTE
            .read()
            .clone()
            .unwrap_or_else(|| "System Output".to_string()),
        sample_rates: vec![44100.0, 48000.0],
        platform_id: 0,
        kind: Default::default(),
    }
}

impl AudioBackend for IosAudioBackend {
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        Ok(vec![current_route()])
    }

    fn default_device(&self) -> Result<DeviceInfo, BackendError> {
        Ok(current_route())
    }

    fn supported_sample_rates(&self, _device: &DeviceInfo) -> Result<Vec<f64>, BackendError> {
        Ok(current_route().sample_rates)
    }

    fn get_device_sample_rate(&self, _device: &DeviceInfo) -> Result<f64, BackendError> {
        // Zero until the session has first been activated: unknown, and no
        // switch to wait out.
        Ok(f64::from_bits(GRANTED.load(Ordering::Acquire)))
    }

    fn set_device_sample_rate(&self, _device: &DeviceInfo, rate: f64) -> Result<f64, BackendError> {
        // A held session takes the new preference at once, and answers for
        // it. One let go is asked when the engine starts, without taking the
        // speaker from another app before anything plays; until then the rate
        // asked for stands in, and the watch corrects it.
        let session = SESSION.read().clone();
        match session {
            Some(session) if session_held() => activate(session.as_ref(), rate).ok_or_else(|| {
                BackendError::Platform("the audio session could not be activated".into())
            }),
            _ => Ok(rate),
        }
    }

    fn watch_device_sample_rate(
        &self,
        _device: &DeviceInfo,
        on_change: Box<dyn Fn(f64) + Send + Sync>,
    ) -> Option<Box<dyn SampleRateWatch>> {
        let watch: RateWatch = Arc::from(on_change);
        *WATCH.lock() = Some(watch.clone());
        Some(Box::new(Watching(watch)))
    }

    fn create_engine(
        &self,
        _device: &DeviceInfo,
        sample_rate: f64,
        channels: u32,
        consumer: rtrb::Consumer<f32>,
        samples_played: Arc<AtomicU64>,
    ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
        let engine = engine::AudioEngine::new(0, sample_rate, channels, consumer, samples_played)
            .map_err(|e| BackendError::StreamCreation(e.to_string()))?;
        Ok(Box::new(engine))
    }
}

/// The player's subscription to the granted rate. A newer one replaces it, so
/// it clears the slot on drop only if it is still the one there.
struct Watching(RateWatch);

impl SampleRateWatch for Watching {}

impl Drop for Watching {
    fn drop(&mut self) {
        let mut slot = WATCH.lock();
        if slot.as_ref().is_some_and(|w| Arc::ptr_eq(w, &self.0)) {
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session on a route that runs at 48 kHz unless asked for 44.1, as a
    /// DAC with only those two would.
    struct TwoRateDac {
        activations: AtomicU64,
        refuse: AtomicBool,
        /// Unplugged: the speaker, at 48 kHz whatever is asked.
        unplugged: AtomicBool,
    }

    impl AudioSession for TwoRateDac {
        fn activate(&self, sample_rate: f64) -> Option<f64> {
            if self.refuse.load(Ordering::Relaxed) {
                return None;
            }
            self.activations.fetch_add(1, Ordering::Relaxed);
            Some(
                if sample_rate == 44100.0 && !self.unplugged.load(Ordering::Relaxed) {
                    44100.0
                } else {
                    48000.0
                },
            )
        }
        fn release(&self) {}
    }

    #[test]
    fn the_rate_reported_is_the_one_the_session_grants() {
        let dac = Arc::new(TwoRateDac {
            activations: AtomicU64::new(0),
            refuse: AtomicBool::new(false),
            unplugged: AtomicBool::new(false),
        });
        set_session(dac.clone());
        let backend = IosAudioBackend;
        let device = backend.default_device().unwrap();
        let heard = Arc::new(AtomicU64::new(0));
        let watch = {
            let heard = heard.clone();
            backend.watch_device_sample_rate(
                &device,
                Box::new(move |rate| heard.store(rate.to_bits(), Ordering::Relaxed)),
            )
        };

        // Not yet held: nothing is activated before the engine starts, and
        // the request stands in until it does.
        assert_eq!(
            backend.set_device_sample_rate(&device, 96000.0).unwrap(),
            96000.0
        );
        assert_eq!(dac.activations.load(Ordering::Relaxed), 0);

        // A refusal, as during a call: not held, nothing heard, and the
        // engine is told not to start.
        dac.refuse.store(true, Ordering::Relaxed);
        assert!(!activate_session(96000.0));
        assert!(!session_held());
        assert_eq!(heard.load(Ordering::Relaxed), 0);
        dac.refuse.store(false, Ordering::Relaxed);

        // Starting takes the session, and the player hears what it runs at.
        assert!(activate_session(96000.0));
        assert_eq!(f64::from_bits(heard.load(Ordering::Relaxed)), 48000.0);
        assert_eq!(backend.get_device_sample_rate(&device).unwrap(), 48000.0);

        // Held: a new track's rate is asked for and answered at once.
        assert_eq!(
            backend.set_device_sample_rate(&device, 44100.0).unwrap(),
            44100.0
        );
        assert_eq!(backend.get_device_sample_rate(&device).unwrap(), 44100.0);

        // Refused while held: an error, so the player keeps the rate it had.
        dac.refuse.store(true, Ordering::Relaxed);
        assert!(backend.set_device_sample_rate(&device, 96000.0).is_err());
        assert_eq!(backend.get_device_sample_rate(&device).unwrap(), 44100.0);
        dac.refuse.store(false, Ordering::Relaxed);

        // The DAC pulled out: the speaker is asked for the same rate, and
        // the player hears the one it runs at. Plugged back in, the DAC is
        // switched to the track's rate again.
        backend.set_device_sample_rate(&device, 44100.0).unwrap();
        dac.unplugged.store(true, Ordering::Relaxed);
        follow_route();
        assert_eq!(f64::from_bits(heard.load(Ordering::Relaxed)), 48000.0);
        assert_eq!(backend.get_device_sample_rate(&device).unwrap(), 48000.0);
        dac.unplugged.store(false, Ordering::Relaxed);
        follow_route();
        assert_eq!(f64::from_bits(heard.load(Ordering::Relaxed)), 44100.0);

        // A session let go is not taken back for a route change.
        release_session();
        let before = dac.activations.load(Ordering::Relaxed);
        follow_route();
        assert_eq!(dac.activations.load(Ordering::Relaxed), before);

        // A replaced watch does not unsubscribe its successor.
        let newer = backend.watch_device_sample_rate(&device, Box::new(|_| {}));
        drop(watch);
        assert!(WATCH.lock().is_some());
        drop(newer);
        assert!(WATCH.lock().is_none());
        release_session();
    }
}
