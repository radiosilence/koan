use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use super::backend::{AudioBackend, AudioEngineHandle, BackendError, DeviceInfo};
use super::{device, engine};

/// CoreAudio AUHAL backend for macOS. Wraps the existing `engine.rs` and
/// `device.rs` FFI code behind the `AudioBackend` trait.
pub struct CoreAudioBackend;

impl CoreAudioBackend {
    fn device_info_from_audio_device(dev: &device::AudioDevice) -> DeviceInfo {
        DeviceInfo {
            name: dev.name.clone(),
            sample_rates: dev.sample_rates.clone(),
            platform_id: u64::from(dev.id),
        }
    }
}

impl AudioBackend for CoreAudioBackend {
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        let devices =
            device::list_output_devices().map_err(|e| BackendError::Platform(e.to_string()))?;
        Ok(devices
            .iter()
            .map(Self::device_info_from_audio_device)
            .collect())
    }

    fn default_device(&self) -> Result<DeviceInfo, BackendError> {
        let id =
            device::default_output_device().map_err(|e| BackendError::Platform(e.to_string()))?;
        let devices =
            device::list_output_devices().map_err(|e| BackendError::Platform(e.to_string()))?;
        devices
            .iter()
            .find(|d| d.id == id)
            .map(Self::device_info_from_audio_device)
            .ok_or(BackendError::NoDevices)
    }

    fn supported_sample_rates(&self, device: &DeviceInfo) -> Result<Vec<f64>, BackendError> {
        let id = device.platform_id as u32;
        device::available_sample_rates(id).map_err(|e| BackendError::Platform(e.to_string()))
    }

    fn get_device_sample_rate(&self, device: &DeviceInfo) -> Result<f64, BackendError> {
        let id = device.platform_id as u32;
        device::get_device_sample_rate(id).map_err(|e| BackendError::Platform(e.to_string()))
    }

    fn set_device_sample_rate(&self, device: &DeviceInfo, rate: f64) -> Result<(), BackendError> {
        let id = device.platform_id as u32;
        device::set_device_sample_rate(id, rate).map_err(|e| BackendError::Platform(e.to_string()))
    }

    fn create_engine(
        &self,
        device: &DeviceInfo,
        sample_rate: f64,
        channels: u32,
        consumer: rtrb::Consumer<f32>,
        samples_played: Arc<AtomicU64>,
    ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
        let id = device.platform_id as u32;
        let engine = engine::AudioEngine::new(id, sample_rate, channels, consumer, samples_played)
            .map_err(|e| BackendError::StreamCreation(e.to_string()))?;
        Ok(Box::new(CoreAudioEngineHandle { engine }))
    }
}

/// Wraps `engine::AudioEngine` behind the `AudioEngineHandle` trait.
struct CoreAudioEngineHandle {
    engine: engine::AudioEngine,
}

// SAFETY: engine::AudioEngine is already Send (unsafe impl Send in engine.rs).
// CoreAudioEngineHandle is a thin wrapper with single ownership — never shared.
unsafe impl Send for CoreAudioEngineHandle {}

impl AudioEngineHandle for CoreAudioEngineHandle {
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
