use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use super::backend::{AudioBackend, AudioEngineHandle, BackendError, DeviceInfo};
use super::engine;

/// iOS output, through RemoteIO.
///
/// The engine is the same one macOS uses — see `engine.rs`. What differs is
/// everything around it, and mostly by subtraction: iOS has no device list, no
/// nominal sample rate to set, and no exclusive access. `AudioHardware.h` is
/// not in the SDK at all, so this is not a matter of writing the FFI.
///
/// What stands in for a device is the audio session's current route, which the
/// host app owns: it decides the category, activates the session, asks for a
/// preferred sample rate, and handles interruptions and route changes. That has
/// to live where there is a run loop and an app lifecycle, so it is Swift's,
/// and this backend deliberately knows nothing about it.
///
/// The consequence worth stating plainly: output is not bit-perfect here. The
/// preferred rate is a request the system may decline, and everything crosses
/// the system mixer whatever it answers.
pub struct IosAudioBackend;

/// The name of the port the session is routed to, as the app last heard it:
/// "AirPods Pro", "Headphones", "Speaker". What DSP profiles are keyed on.
static ROUTE: parking_lot::RwLock<Option<String>> = parking_lot::RwLock::new(None);

/// Told by the app on each route change.
pub fn set_route(name: String) {
    *ROUTE.write() = Some(name);
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
        // The session knows, and the session is the app's. Answering with the
        // source rate keeps the player from trying to switch to something else.
        Ok(0.0)
    }

    fn set_device_sample_rate(&self, _device: &DeviceInfo, rate: f64) -> Result<f64, BackendError> {
        // Nothing to set: `AVAudioSession.setPreferredSampleRate` is the only
        // lever and it belongs to the app. Reporting the rate back unchanged
        // says "asked for, not guaranteed", which is the truth on iOS.
        Ok(rate)
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
