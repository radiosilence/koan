use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("no output devices found")]
    NoDevices,
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("unsupported sample rate: {0}")]
    UnsupportedSampleRate(f64),
    #[error("platform error: {0}")]
    Platform(String),
    #[error("stream creation failed: {0}")]
    StreamCreation(String),
}

/// Platform-agnostic output device descriptor.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub sample_rates: Vec<f64>,
    /// Opaque platform-specific ID. CoreAudio: AudioDeviceID, cpal: index.
    pub platform_id: u64,
}

/// Trait abstracting platform audio output.
///
/// Implementations exist for CoreAudio (macOS) and cpal (Linux).
/// The decode pipeline (rtrb ring buffer, Symphonia, `PlaybackTimeline`) is
/// completely decoupled — backends are dumb consumers that drain the ring buffer.
pub trait AudioBackend: Send + Sync {
    /// List available output devices.
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, BackendError>;

    /// Get the default output device.
    fn default_device(&self) -> Result<DeviceInfo, BackendError>;

    /// Query supported sample rates for a device.
    fn supported_sample_rates(&self, device: &DeviceInfo) -> Result<Vec<f64>, BackendError>;

    /// Get the current nominal sample rate of a device.
    fn get_device_sample_rate(&self, device: &DeviceInfo) -> Result<f64, BackendError>;

    /// Set the nominal sample rate of a device (for bit-perfect matching).
    /// Returns the actual device rate after the switch (may differ if unsupported).
    /// On Linux/cpal this is a no-op — the rate is set at stream creation.
    fn set_device_sample_rate(&self, device: &DeviceInfo, rate: f64) -> Result<f64, BackendError>;

    /// Create an audio engine targeting a device at a specific format.
    /// Takes ownership of the rtrb consumer.
    fn create_engine(
        &self,
        device: &DeviceInfo,
        sample_rate: f64,
        channels: u32,
        consumer: rtrb::Consumer<f32>,
        samples_played: Arc<AtomicU64>,
    ) -> Result<Box<dyn AudioEngineHandle>, BackendError>;
}

/// Handle to a running audio engine. Start/stop control.
pub trait AudioEngineHandle: Send {
    fn start(&self) -> Result<(), BackendError>;
    fn stop(&self) -> Result<(), BackendError>;
    fn is_running(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_info_construction() {
        let info = DeviceInfo {
            name: "Test DAC".into(),
            sample_rates: vec![44100.0, 48000.0, 96000.0],
            platform_id: 42,
        };
        assert_eq!(info.name, "Test DAC");
        assert_eq!(info.sample_rates.len(), 3);
        assert_eq!(info.platform_id, 42);
    }

    #[test]
    fn backend_error_formatting() {
        let err = BackendError::NoDevices;
        assert_eq!(err.to_string(), "no output devices found");

        let err = BackendError::DeviceNotFound("Missing".into());
        assert!(err.to_string().contains("Missing"));

        let err = BackendError::UnsupportedSampleRate(192000.0);
        assert!(err.to_string().contains("192000"));
    }

    #[test]
    fn platform_backend_constructs() {
        // Verify the platform backend can be created without panicking.
        let _backend = super::super::platform_backend();
    }

    #[test]
    fn platform_backend_lists_devices() {
        let backend = super::super::platform_backend();
        // Should not panic. May return empty on CI (no audio hardware).
        let result = backend.list_devices();
        assert!(result.is_ok());
    }

    #[test]
    fn platform_backend_has_default_device() {
        let backend = super::super::platform_backend();
        // On real hardware this should succeed. On CI it might fail (no device).
        // We just verify it doesn't panic.
        let _ = backend.default_device();
    }

    #[test]
    fn engine_create_with_ring_buffer() {
        let backend = super::super::platform_backend();
        let device = match backend.default_device() {
            Ok(d) => d,
            Err(_) => return, // no audio device (CI) — skip
        };

        let (producer, consumer) = rtrb::RingBuffer::new(4096);
        let samples_played = Arc::new(AtomicU64::new(0));

        let rate = device.sample_rates.first().copied().unwrap_or(44100.0);

        let engine = backend.create_engine(&device, rate, 2, consumer, samples_played);
        // Should create without panicking on real hardware.
        // May fail on CI — that's fine, we're testing the code path not the hardware.
        if let Ok(engine) = engine {
            assert!(!engine.is_running());
            // Don't start — no point playing silence in a test.
            drop(engine);
        }
        drop(producer); // keep producer alive until after engine
    }
}
