use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::backend::{AudioBackend, AudioEngineHandle, BackendError, DeviceInfo};

/// Temporarily redirect stderr to /dev/null while running a closure.
/// ALSA/JACK/PipeWire C libraries spam stderr when probing unavailable
/// backends. In TUI mode this corrupts the alternate screen display.
fn suppress_stderr<F: FnOnce() -> T, T>(f: F) -> T {
    use std::os::fd::AsRawFd;

    // dup(2) to save original stderr, dup2(/dev/null, 2) to suppress, restore after.
    let devnull = std::fs::File::open("/dev/null").ok();
    let saved_fd = unsafe { nix_dup(2) };

    if let Some(ref null) = devnull
        && saved_fd >= 0
    {
        unsafe { nix_dup2(null.as_raw_fd(), 2) };
    }

    let result = f();

    if saved_fd >= 0 {
        unsafe {
            nix_dup2(saved_fd, 2);
            nix_close(saved_fd);
        }
    }

    result
}

// Thin wrappers around libc dup/dup2/close — avoids adding libc as a dep.
unsafe fn nix_dup(fd: i32) -> i32 {
    unsafe extern "C" {
        safe fn dup(fd: i32) -> i32;
    }
    dup(fd)
}
unsafe fn nix_dup2(oldfd: i32, newfd: i32) -> i32 {
    unsafe extern "C" {
        safe fn dup2(oldfd: i32, newfd: i32) -> i32;
    }
    dup2(oldfd, newfd)
}
unsafe fn nix_close(fd: i32) -> i32 {
    unsafe extern "C" {
        safe fn close(fd: i32) -> i32;
    }
    close(fd)
}

/// cpal-based audio backend for Linux (ALSA / PipeWire / PulseAudio).
///
/// The callback drains the rtrb consumer identically to the CoreAudio
/// render callback: read available samples, copy to output, zero-pad
/// remainder, increment `samples_played`.
pub struct CpalBackend {
    host: cpal::Host,
}

impl Default for CpalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CpalBackend {
    pub fn new() -> Self {
        // Suppress ALSA/JACK/OSS probe spam on stderr during host initialization.
        // These C libraries write directly to fd 2 when probing unavailable backends
        // (JACK not running, OSS /dev/dsp missing, etc.). In TUI mode this bleeds
        // through the alternate screen and corrupts the display.
        let host = suppress_stderr(cpal::default_host);
        Self { host }
    }

    /// Resolve a `DeviceInfo` back to a cpal `Device` by matching name.
    fn resolve_device(&self, info: &DeviceInfo) -> Result<cpal::Device, BackendError> {
        let devices = self
            .host
            .output_devices()
            .map_err(|e| BackendError::Platform(e.to_string()))?;

        for dev in devices {
            if let Ok(desc) = dev.description()
                && desc.name() == info.name
            {
                return Ok(dev);
            }
        }

        Err(BackendError::DeviceNotFound(info.name.clone()))
    }

    fn device_info_from_cpal(dev: &cpal::Device, index: u64) -> Option<DeviceInfo> {
        let name = dev.description().ok()?.name().to_owned();
        let configs = dev.supported_output_configs().ok()?;
        let mut rates: Vec<f64> = Vec::new();
        for cfg in configs {
            // Collect min and max sample rates from each config range.
            let min = cfg.min_sample_rate() as f64;
            let max = cfg.max_sample_rate() as f64;
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
        let devices = suppress_stderr(|| self.host.output_devices())
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
        let dev =
            suppress_stderr(|| self.host.default_output_device()).ok_or(BackendError::NoDevices)?;
        suppress_stderr(|| Self::device_info_from_cpal(&dev, 0)).ok_or(BackendError::NoDevices)
    }

    fn supported_sample_rates(&self, device: &DeviceInfo) -> Result<Vec<f64>, BackendError> {
        Ok(device.sample_rates.clone())
    }

    fn get_device_sample_rate(&self, device: &DeviceInfo) -> Result<f64, BackendError> {
        let dev = self.resolve_device(device)?;
        let config = suppress_stderr(|| dev.default_output_config())
            .map_err(|e| BackendError::Platform(e.to_string()))?;
        Ok(config.sample_rate() as f64)
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
            sample_rate: sample_rate as u32,
            buffer_size: cpal::BufferSize::Default,
        };

        let running = Arc::new(AtomicBool::new(false));
        let running_cb = running.clone();

        // Wrap consumer in a Mutex so we can move it into the FnMut callback.
        // The callback runs on a single audio thread so contention is zero.
        let consumer = std::sync::Mutex::new(consumer);

        let stream = suppress_stderr(|| {
            dev.build_output_stream(
                &config,
                move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                    if !running_cb.load(Ordering::Acquire) {
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

                    if to_read > 0
                        && let Ok(chunk) = consumer.read_chunk(to_read)
                    {
                        let (first, second) = chunk.as_slices();
                        let ring_total = first.len() + second.len();
                        let copy_total = ring_total.min(total_samples);
                        let first_copy = first.len().min(copy_total);
                        let second_copy = copy_total.saturating_sub(first_copy).min(second.len());
                        unsafe {
                            ptr::copy_nonoverlapping(first.as_ptr(), data.as_mut_ptr(), first_copy);
                            if second_copy > 0 {
                                ptr::copy_nonoverlapping(
                                    second.as_ptr(),
                                    data.as_mut_ptr().add(first_copy),
                                    second_copy,
                                );
                            }
                        }
                        chunk.commit_all();
                        samples_played.fetch_add(copy_total as u64, Ordering::AcqRel);
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
            .map_err(|e| BackendError::StreamCreation(e.to_string()))
        })?;

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
        self.running.store(true, Ordering::Release);
        self.stream
            .play()
            .map_err(|e| BackendError::Platform(e.to_string()))
    }

    fn stop(&self) -> Result<(), BackendError> {
        self.running.store(false, Ordering::Release);
        self.stream
            .pause()
            .map_err(|e| BackendError::Platform(e.to_string()))
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
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
