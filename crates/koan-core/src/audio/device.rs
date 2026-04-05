use std::mem;
use std::os::raw::c_void;
use std::ptr;
use std::sync::mpsc;
use std::time::Duration;

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use coreaudio_sys::*;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("CoreAudio error: {0}")]
    OSStatus(i32),
    #[error("no output devices found")]
    NoDevices,
    #[error("device not found: {0}")]
    NotFound(AudioDeviceID),
    #[error("device not found by name: {0}")]
    NotFoundByName(String),
}

type Result<T> = std::result::Result<T, DeviceError>;

fn check(status: OSStatus) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(DeviceError::OSStatus(status))
    }
}

#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub id: AudioDeviceID,
    pub name: String,
    pub sample_rates: Vec<f64>,
}

/// Get the default output device ID.
pub fn default_output_device() -> Result<AudioDeviceID> {
    let property = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyDefaultOutputDevice,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };

    let mut device_id: AudioDeviceID = 0;
    let mut size = mem::size_of::<AudioDeviceID>() as u32;

    // SAFETY: `device_id` is a stack-allocated AudioDeviceID with correct size.
    // CoreAudio writes a single u32 device ID into the provided buffer.
    check(unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &property,
            0,
            ptr::null(),
            &mut size,
            &mut device_id as *mut _ as *mut _,
        )
    })?;

    if device_id == 0 {
        return Err(DeviceError::NoDevices);
    }

    Ok(device_id)
}

/// List all output devices.
pub fn list_output_devices() -> Result<Vec<AudioDevice>> {
    let property = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyDevices,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };

    let mut size: u32 = 0;
    // SAFETY: Querying data size only — no output buffer, just writes to `size`.
    check(unsafe {
        AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject,
            &property,
            0,
            ptr::null(),
            &mut size,
        )
    })?;

    let device_count = size as usize / mem::size_of::<AudioDeviceID>();
    let mut device_ids = vec![0u32; device_count];

    // SAFETY: `device_ids` is pre-allocated to exactly the size returned by
    // GetPropertyDataSize. CoreAudio fills it with `device_count` AudioDeviceIDs.
    check(unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &property,
            0,
            ptr::null(),
            &mut size,
            device_ids.as_mut_ptr() as *mut _,
        )
    })?;

    let mut devices = Vec::new();
    for id in device_ids {
        if !has_output_streams(id) {
            continue;
        }
        let name = device_name(id).unwrap_or_else(|_| format!("Unknown ({})", id));
        let sample_rates = available_sample_rates(id).unwrap_or_default();
        devices.push(AudioDevice {
            id,
            name,
            sample_rates,
        });
    }

    Ok(devices)
}

/// Check if a device has output streams.
fn has_output_streams(device_id: AudioDeviceID) -> bool {
    let property = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyStreams,
        mScope: kAudioObjectPropertyScopeOutput,
        mElement: kAudioObjectPropertyElementMain,
    };

    let mut size: u32 = 0;
    // SAFETY: Querying data size only — no output buffer, just writes to `size`.
    let status =
        unsafe { AudioObjectGetPropertyDataSize(device_id, &property, 0, ptr::null(), &mut size) };

    status == 0 && size > 0
}

/// Get a device's name via CFString.
fn device_name(device_id: AudioDeviceID) -> Result<String> {
    let property = AudioObjectPropertyAddress {
        mSelector: kAudioObjectPropertyName,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };

    let mut name_ref: CFStringRef = ptr::null();
    let mut size = mem::size_of::<CFStringRef>() as u32;

    // SAFETY: Standard CoreAudio property query. `name_ref` is a correctly-sized
    // output buffer for a single CFStringRef. CoreAudio writes the pointer and
    // transfers ownership to the caller (Create Rule).
    check(unsafe {
        AudioObjectGetPropertyData(
            device_id,
            &property,
            0,
            ptr::null(),
            &mut size,
            &mut name_ref as *mut _ as *mut _,
        )
    })?;

    if name_ref.is_null() {
        return Ok(String::new());
    }

    // SAFETY: `name_ref` was returned by a CoreAudio Create Rule API — the caller
    // owns the reference. `wrap_under_create_rule` takes ownership and will
    // CFRelease on drop, so no manual release is needed. The pointer cast bridges
    // coreaudio-sys's CFStringRef and core-foundation's CFStringRef which are
    // identical C types from different bindgen runs.
    let cf_string: CFString = unsafe {
        CFString::wrap_under_create_rule(name_ref as core_foundation::string::CFStringRef)
    };
    Ok(cf_string.to_string())
}

/// Get available sample rates for a device.
pub fn available_sample_rates(device_id: AudioDeviceID) -> Result<Vec<f64>> {
    let property = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyAvailableNominalSampleRates,
        mScope: kAudioObjectPropertyScopeOutput,
        mElement: kAudioObjectPropertyElementMain,
    };

    let mut size: u32 = 0;
    // SAFETY: Querying data size only — no output buffer, just writes to `size`.
    check(unsafe {
        AudioObjectGetPropertyDataSize(device_id, &property, 0, ptr::null(), &mut size)
    })?;

    let count = size as usize / mem::size_of::<AudioValueRange>();
    let mut ranges = vec![
        AudioValueRange {
            mMinimum: 0.0,
            mMaximum: 0.0,
        };
        count
    ];

    // SAFETY: `ranges` is pre-allocated to exactly the size returned by
    // GetPropertyDataSize. CoreAudio fills it with `count` AudioValueRange structs.
    check(unsafe {
        AudioObjectGetPropertyData(
            device_id,
            &property,
            0,
            ptr::null(),
            &mut size,
            ranges.as_mut_ptr() as *mut _,
        )
    })?;

    let mut rates: Vec<f64> = ranges.iter().map(|r| r.mMaximum).collect();
    rates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    rates.dedup();

    Ok(rates)
}

/// Get the current nominal sample rate of a device.
pub fn get_device_sample_rate(device_id: AudioDeviceID) -> Result<f64> {
    let property = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyNominalSampleRate,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };

    let mut rate: f64 = 0.0;
    let mut size = mem::size_of::<f64>() as u32;

    // SAFETY: `rate` is a stack-allocated f64 with correct size for the property.
    check(unsafe {
        AudioObjectGetPropertyData(
            device_id,
            &property,
            0,
            ptr::null(),
            &mut size,
            &mut rate as *mut _ as *mut _,
        )
    })?;

    Ok(rate)
}

/// Find an output device by name. Returns the device ID if found.
pub fn find_output_device_by_name(name: &str) -> Result<AudioDeviceID> {
    let devices = list_output_devices()?;
    devices
        .iter()
        .find(|d| d.name == name)
        .map(|d| d.id)
        .ok_or_else(|| DeviceError::NotFoundByName(name.to_string()))
}

/// RAII guard that removes a CoreAudio property listener on drop.
/// Prevents listener registration leaks even on early returns or panics.
struct ListenerGuard {
    device_id: AudioDeviceID,
    property: AudioObjectPropertyAddress,
    callback: AudioObjectPropertyListenerProc,
    client_data: *mut c_void,
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        // SAFETY: Removing the same listener+client_data pair that was registered.
        // Must happen exactly once — guaranteed by the Drop trait.
        unsafe {
            AudioObjectRemovePropertyListener(
                self.device_id,
                &self.property,
                self.callback,
                self.client_data,
            );
        }
    }
}

/// CoreAudio property listener callback. Fires on the HAL I/O thread when the
/// nominal sample rate changes. Zero work — just signals the waiting thread.
///
/// # Safety
/// `in_client_data` must point to a valid `mpsc::SyncSender<()>`. The pointer
/// is valid for the lifetime of the `ListenerGuard` in `set_device_sample_rate`.
unsafe extern "C" fn rate_change_callback(
    _in_object_id: AudioObjectID,
    _in_number_addresses: UInt32,
    _in_addresses: *const AudioObjectPropertyAddress,
    in_client_data: *mut c_void,
) -> OSStatus {
    // SAFETY: `in_client_data` points to a valid `SyncSender<()>` on the calling
    // thread's stack, kept alive by the `ListenerGuard` which removes this listener
    // before the sender is dropped.
    let tx = unsafe { &*(in_client_data as *const mpsc::SyncSender<()>) };
    // Ignore send errors — receiver may have already been dropped (timeout).
    let _ = tx.try_send(());
    0
}

/// Set the nominal sample rate of a device (for bit-perfect matching).
///
/// CoreAudio rate changes are asynchronous — the device needs time to physically
/// reclock. This function registers a property listener on
/// `kAudioDevicePropertyNominalSampleRate` and waits for the callback instead of
/// polling. Falls back on timeout (5s, covers slow USB Class 1 DACs doing PLL
/// relock). Returns the actual device rate after the attempt.
pub fn set_device_sample_rate(device_id: AudioDeviceID, rate: f64) -> Result<f64> {
    let property = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyNominalSampleRate,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };

    // Early-out: if the device already reports the target rate, skip the switch.
    let current = get_device_sample_rate(device_id)?;
    if (current - rate).abs() < 0.1 {
        return Ok(current);
    }

    // Set up a oneshot channel for the property listener callback.
    let (tx, rx) = mpsc::sync_channel::<()>(1);

    // SAFETY: `tx` lives on the stack for the duration of this function.
    // The ListenerGuard removes the listener before `tx` is dropped, so the
    // callback never sees a dangling pointer.
    let client_data = &tx as *const mpsc::SyncSender<()> as *mut c_void;

    check(unsafe {
        AudioObjectAddPropertyListener(
            device_id,
            &property,
            Some(rate_change_callback),
            client_data,
        )
    })?;

    // RAII: ensures RemovePropertyListener runs even on early return/panic.
    let _guard = ListenerGuard {
        device_id,
        property,
        callback: Some(rate_change_callback),
        client_data,
    };

    // Request the rate change.
    // SAFETY: `rate` is a stack-allocated f64 with correct size for the property.
    check(unsafe {
        AudioObjectSetPropertyData(
            device_id,
            &property,
            0,
            ptr::null(),
            mem::size_of::<f64>() as u32,
            &rate as *const _ as *const _,
        )
    })?;

    // Wait for the listener callback or timeout.
    // 5s covers USB Class 1 DACs that need PLL relock (3-5s typical).
    const TIMEOUT: Duration = Duration::from_secs(5);

    match rx.recv_timeout(TIMEOUT) {
        Ok(()) => {
            // Callback fired — verify the rate actually matches (spurious callbacks
            // can fire for reasons other than our change).
            let actual = get_device_sample_rate(device_id)?;
            if (actual - rate).abs() < 0.1 {
                return Ok(actual);
            }
            log::warn!(
                "sample rate listener fired but rate mismatch: requested {rate}Hz, device reports {actual}Hz"
            );
            Ok(actual)
        }
        Err(_) => {
            let actual = get_device_sample_rate(device_id)?;
            log::warn!(
                "device sample rate switch timed out after {TIMEOUT:?}: requested {rate}Hz, device reports {actual}Hz"
            );
            Ok(actual)
        }
    }
    // _guard drops here → AudioObjectRemovePropertyListener called before tx is dropped
}
