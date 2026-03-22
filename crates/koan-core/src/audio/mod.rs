pub mod analyzer;
pub mod backend;
pub mod buffer;
#[cfg(target_os = "macos")]
pub mod coreaudio_backend;
#[cfg(target_os = "linux")]
pub mod cpal_backend;
#[cfg(target_os = "macos")]
pub mod device;
#[cfg(target_os = "macos")]
pub mod engine;
pub mod replaygain;
pub mod streaming;
pub mod viz;

use backend::{AudioBackend, BackendError, DeviceInfo};

/// Construct the platform-appropriate audio backend.
pub fn platform_backend() -> Box<dyn AudioBackend> {
    #[cfg(target_os = "macos")]
    {
        Box::new(coreaudio_backend::CoreAudioBackend)
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(cpal_backend::CpalBackend::new())
    }
}

/// Cross-platform facade: list output devices via the platform backend.
pub fn list_output_devices() -> Result<Vec<DeviceInfo>, BackendError> {
    platform_backend().list_devices()
}

/// Cross-platform facade: get the default output device.
pub fn default_output_device() -> Result<DeviceInfo, BackendError> {
    platform_backend().default_device()
}
