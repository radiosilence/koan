use std::mem;
use std::ptr;

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

    let name = unsafe { cfstring_to_string(name_ref) };
    if !name_ref.is_null() {
        unsafe { CFRelease(name_ref as *const _) };
    }
    Ok(name)
}

/// Convert a CFStringRef to a Rust String.
unsafe fn cfstring_to_string(cf_str: CFStringRef) -> String {
    if cf_str.is_null() {
        return String::new();
    }

    let len = unsafe { CFStringGetLength(cf_str) };
    let mut buf = vec![0u8; (len as usize) * 4];
    let mut used: CFIndex = 0;

    unsafe {
        CFStringGetBytes(
            cf_str,
            CFRange {
                location: 0,
                length: len,
            },
            kCFStringEncodingUTF8,
            0,
            false as Boolean,
            buf.as_mut_ptr(),
            buf.len() as CFIndex,
            &mut used,
        );
    }

    buf.truncate(used as usize);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Get available sample rates for a device.
pub fn available_sample_rates(device_id: AudioDeviceID) -> Result<Vec<f64>> {
    let property = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyAvailableNominalSampleRates,
        mScope: kAudioObjectPropertyScopeOutput,
        mElement: kAudioObjectPropertyElementMain,
    };

    let mut size: u32 = 0;
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

/// Set the nominal sample rate of a device (for bit-perfect matching).
pub fn set_device_sample_rate(device_id: AudioDeviceID, rate: f64) -> Result<()> {
    let property = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyNominalSampleRate,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };

    check(unsafe {
        AudioObjectSetPropertyData(
            device_id,
            &property,
            0,
            ptr::null(),
            mem::size_of::<f64>() as u32,
            &rate as *const _ as *const _,
        )
    })
}
