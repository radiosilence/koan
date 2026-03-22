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
    /// On Linux/cpal this is a no-op — the rate is set at stream creation.
    fn set_device_sample_rate(&self, device: &DeviceInfo, rate: f64) -> Result<(), BackendError>;

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
