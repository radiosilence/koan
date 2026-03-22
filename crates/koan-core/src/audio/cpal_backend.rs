use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::backend::{AudioBackend, AudioEngineHandle, BackendError, DeviceInfo};

/// cpal-based audio backend for Linux (ALSA / PipeWire / PulseAudio).
///
/// The callback drains the rtrb consumer identically to the CoreAudio
/// render callback: read available samples, copy to output, zero-pad
/// remainder, increment `samples_played`.
pub struct CpalBackend {
    host: cpal::Host,
}

impl CpalBackend {
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }

    /// Resolve a `DeviceInfo` back to a cpal `Device` by matching name.
    fn resolve_device(&self, info: &DeviceInfo) -> Result<cpal::Device, BackendError> {
        let devices = self
            .host
            .output_devices()
            .map_err(|e| BackendError::Platform(e.to_string()))?;

        for dev in devices {
            if let Ok(name) = dev.name() {
                if name == info.name {
                    return Ok(dev);
                }
            }
        }

        Err(BackendError::DeviceNotFound(info.name.clone()))
    }

    fn device_info_from_cpal(dev: &cpal::Device, index: u64) -> Option<DeviceInfo> {
        let name = dev.name().ok()?;
        let configs = dev.supported_output_configs().ok()?;
        let mut rates: Vec<f64> = Vec::new();
        for cfg in configs {
            // Collect min and max sample rates from each config range.
            let min = cfg.min_sample_rate().0 as f64;
            let max = cfg.max_sample_rate().0 as f64;
            if !rates.contains(&min) {
                rates.push(min);
            }
            if !rates.contains(&max) {
                rates.push(max);
            }
            // Add common rates that fall within the range.
            for &common in &[44100.0, 48000.0, 88200.0, 96000.0, 176400.0, 192000.0] {
                if common >= min && common <= max && !rates.contains(&common) {
                    rates.push(common);
                }
            }
        }
        rates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        rates.dedup();

        Some(DeviceInfo {
            name,
            sample_rates: rates,
            platform_id: index,
        })
    }
}

impl AudioBackend for CpalBackend {
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        let devices = self
            .host
            .output_devices()
            .map_err(|e| BackendError::Platform(e.to_string()))?;

        let mut result = Vec::new();
        for (i, dev) in devices.enumerate() {
            if let Some(info) = Self::device_info_from_cpal(&dev, i as u64) {
                result.push(info);
            }
        }
        Ok(result)
    }

    fn default_device(&self) -> Result<DeviceInfo, BackendError> {
        let dev = self
            .host
            .default_output_device()
            .ok_or(BackendError::NoDevices)?;
        Self::device_info_from_cpal(&dev, 0).ok_or(BackendError::NoDevices)
    }

    fn supported_sample_rates(&self, device: &DeviceInfo) -> Result<Vec<f64>, BackendError> {
        Ok(device.sample_rates.clone())
    }

    fn get_device_sample_rate(&self, device: &DeviceInfo) -> Result<f64, BackendError> {
        // cpal doesn't expose the device's current nominal rate.
        // Return the first supported rate as a reasonable default.
        let dev = self.resolve_device(device)?;
        let config = dev
            .default_output_config()
            .map_err(|e| BackendError::Platform(e.to_string()))?;
        Ok(config.sample_rate().0 as f64)
    }

    fn set_device_sample_rate(&self, _device: &DeviceInfo, _rate: f64) -> Result<(), BackendError> {
        // On Linux, sample rate is set at stream creation time.
        // This is a no-op — the rate will be applied in `create_engine`.
        Ok(())
    }

    fn create_engine(
        &self,
        device: &DeviceInfo,
        sample_rate: f64,
        channels: u32,
        consumer: rtrb::Consumer<f32>,
        samples_played: Arc<AtomicU64>,
    ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
        let dev = self.resolve_device(device)?;

        let config = cpal::StreamConfig {
            channels: channels as u16,
            sample_rate: cpal::SampleRate(sample_rate as u32),
            buffer_size: cpal::BufferSize::Default,
        };

        let running = Arc::new(AtomicBool::new(false));
        let running_cb = running.clone();

        // Wrap consumer in a Mutex so we can move it into the FnMut callback.
        // The callback runs on a single audio thread so contention is zero.
        let consumer = std::sync::Mutex::new(consumer);

        let stream = dev
            .build_output_stream(
                &config,
                move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                    if !running_cb.load(Ordering::Relaxed) {
                        data.fill(0.0);
                        return;
                    }

                    let Ok(mut consumer) = consumer.lock() else {
                        data.fill(0.0);
                        return;
                    };

                    let total_samples = data.len();
                    let available = consumer.slots();
                    let to_read = available.min(total_samples);

                    if to_read > 0 {
                        if let Ok(chunk) = consumer.read_chunk(to_read) {
                            let (first, second) = chunk.as_slices();
                            // Copy from ring buffer slices into the output buffer.
                            unsafe {
                                ptr::copy_nonoverlapping(
                                    first.as_ptr(),
                                    data.as_mut_ptr(),
                                    first.len(),
                                );
                                if !second.is_empty() {
                                    ptr::copy_nonoverlapping(
                                        second.as_ptr(),
                                        data.as_mut_ptr().add(first.len()),
                                        second.len(),
                                    );
                                }
                            }
                            chunk.commit_all();
                            samples_played.fetch_add(to_read as u64, Ordering::Relaxed);
                        }
                    }

                    // Zero-pad remainder on underrun — silence > glitches.
                    if to_read < total_samples {
                        data[to_read..].fill(0.0);
                    }
                },
                |err| {
                    log::error!("cpal audio stream error: {}", err);
                },
                None,
            )
            .map_err(|e| BackendError::StreamCreation(e.to_string()))?;

        Ok(Box::new(CpalEngineHandle { stream, running }))
    }
}

/// Handle to a cpal output stream.
struct CpalEngineHandle {
    stream: cpal::Stream,
    running: Arc<AtomicBool>,
}

// SAFETY: cpal::Stream is Send on all platforms cpal supports.
// The stream callback captures its own state and is invoked by cpal's audio thread.
unsafe impl Send for CpalEngineHandle {}

impl AudioEngineHandle for CpalEngineHandle {
    fn start(&self) -> Result<(), BackendError> {
        self.running.store(true, Ordering::Relaxed);
        self.stream
            .play()
            .map_err(|e| BackendError::Platform(e.to_string()))
    }

    fn stop(&self) -> Result<(), BackendError> {
        self.running.store(false, Ordering::Relaxed);
        self.stream
            .pause()
            .map_err(|e| BackendError::Platform(e.to_string()))
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpal_backend_constructs() {
        let _backend = CpalBackend::new();
    }

    #[test]
    fn cpal_list_devices_no_panic() {
        let backend = CpalBackend::new();
        let result = backend.list_devices();
        assert!(result.is_ok());
    }

    #[test]
    fn cpal_set_sample_rate_is_noop() {
        let backend = CpalBackend::new();
        let dummy = DeviceInfo {
            name: "nonexistent".into(),
            sample_rates: vec![44100.0],
            platform_id: 0,
        };
        assert!(backend.set_device_sample_rate(&dummy, 96000.0).is_ok());
    }

    #[test]
    fn cpal_device_info_has_sample_rates() {
        let backend = CpalBackend::new();
        if let Ok(devices) = backend.list_devices() {
            for dev in &devices {
                if !dev.sample_rates.is_empty() {
                    assert!(dev.sample_rates[0] > 0.0);
                }
            }
        }
    }

    #[test]
    fn cpal_resolve_nonexistent_device_fails() {
        let backend = CpalBackend::new();
        let dummy = DeviceInfo {
            name: "this device does not exist xyzzy".into(),
            sample_rates: vec![],
            platform_id: 9999,
        };
        assert!(backend.resolve_device(&dummy).is_err());
    }
}
