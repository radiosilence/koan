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

/// The one device there is: whatever the session is routed to.
///
/// Named rather than enumerated, because the name is all iOS will tell us
/// without going through the session — and the session is the app's.
fn current_route() -> DeviceInfo {
    DeviceInfo {
        name: "System Output".to_string(),
        sample_rates: vec![44100.0, 48000.0],
        platform_id: 0,
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
        Ok(Box::new(IosEngineHandle { engine }))
    }
}

struct IosEngineHandle {
    engine: engine::AudioEngine,
}

// SAFETY: as on macOS — `engine::AudioEngine` is already `Send`, and this is a
// thin wrapper with a single owner.
unsafe impl Send for IosEngineHandle {}

impl AudioEngineHandle for IosEngineHandle {
    fn start(&self) -> Result<(), BackendError> {
        self.engine
            .start()
            .map_err(|e| BackendError::Platform(e.to_string()))
    }

    fn stop(&self) -> Result<(), BackendError> {
        self.engine
            .stop()
            .map_err(|e| BackendError::Platform(e.to_string()))
    }

    fn is_running(&self) -> bool {
        self.engine.is_running()
    }
}
