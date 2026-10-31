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
    pub kind: super::backend::OutputKind,
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
            kind: transport_kind(id),
        });
    }

    Ok(devices)
}

/// How a device is connected.
fn transport_kind(device_id: AudioDeviceID) -> super::backend::OutputKind {
    use super::backend::OutputKind;
    let property = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyTransportType,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut transport: u32 = 0;
    let mut size = mem::size_of::<u32>() as u32;
    // SAFETY: Standard CoreAudio property query into a correctly sized u32.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device_id,
            &property,
            0,
            ptr::null(),
            &mut size,
            &mut transport as *mut u32 as *mut _,
        )
    };
    if status != 0 {
        return OutputKind::Other;
    }
    #[allow(non_upper_case_globals)]
    match transport {
        kAudioDeviceTransportTypeBuiltIn => OutputKind::BuiltIn,
        kAudioDeviceTransportTypeUSB => OutputKind::Usb,
        kAudioDeviceTransportTypeBluetooth | kAudioDeviceTransportTypeBluetoothLE => {
            OutputKind::Bluetooth
        }
        kAudioDeviceTransportTypeAirPlay => OutputKind::AirPlay,
        kAudioDeviceTransportTypeHDMI | kAudioDeviceTransportTypeDisplayPort => OutputKind::Display,
        kAudioDeviceTransportTypeVirtual | kAudioDeviceTransportTypeAggregate => {
            OutputKind::Virtual
        }
        _ => OutputKind::Other,
    }
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

/// What each registered rate listener does, by the id CoreAudio is handed in
/// place of a pointer. Removing a listener does not wait for a call already
/// running on the HAL's notification thread, so that call must not reach
/// anything the remover then frees: it finds the handler here, or nothing.
type Handler = std::sync::Arc<dyn Fn() + Send + Sync>;
static HANDLERS: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<usize, Handler>>,
> = std::sync::LazyLock::new(Default::default);
static NEXT_HANDLER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

/// A nominal-rate listener on a device, removed on drop.
struct ListenerGuard {
    device_id: AudioDeviceID,
    property: AudioObjectPropertyAddress,
    id: usize,
}

impl ListenerGuard {
    fn add(device_id: AudioDeviceID, handler: Handler) -> Result<Self> {
        let property = AudioObjectPropertyAddress {
            mSelector: kAudioDevicePropertyNominalSampleRate,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };
        let id = NEXT_HANDLER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        HANDLERS.lock().insert(id, handler);
        // SAFETY: the client data is an id, never dereferenced.
        let added = check(unsafe {
            AudioObjectAddPropertyListener(
                device_id,
                &property,
                Some(rate_listener),
                id as *mut c_void,
            )
        });
        if let Err(e) = added {
            HANDLERS.lock().remove(&id);
            return Err(e);
        }
        Ok(Self {
            device_id,
            property,
            id,
        })
    }
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        // SAFETY: removes the listener+id pair registered in `add`, once.
        unsafe {
            AudioObjectRemovePropertyListener(
                self.device_id,
                &self.property,
                Some(rate_listener),
                self.id as *mut c_void,
            );
        }
        HANDLERS.lock().remove(&self.id);
    }
}

/// Fires on the HAL notification thread when a device's nominal rate moves.
/// Runs the handler registered under the id it is given, if it is still
/// registered; the lock is not held while it runs.
unsafe extern "C" fn rate_listener(
    _in_object_id: AudioObjectID,
    _in_number_addresses: UInt32,
    _in_addresses: *const AudioObjectPropertyAddress,
    in_client_data: *mut c_void,
) -> OSStatus {
    let handler = HANDLERS.lock().get(&(in_client_data as usize)).cloned();
    if let Some(handler) = handler {
        handler();
    }
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

    // Signalled by the listener when the rate moves.
    let (tx, rx) = mpsc::sync_channel::<()>(1);
    let _guard = ListenerGuard::add(
        device_id,
        std::sync::Arc::new(move || {
            // Ignore send errors: the receiver may have timed out.
            let _ = tx.try_send(());
        }),
    )?;

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
}

/// A live subscription to a device's nominal sample rate.
///
/// The device rate is shared: koan sets it, but Audio MIDI Setup, a vendor
/// control panel or any other client can move it back at any moment, and the
/// HAL then resamples koan to reach it. A rate read once at engine creation
/// goes stale the instant that happens — and the claim it feeds is the one
/// this player exists to make. Dropping the watch unsubscribes.
pub struct RateWatch {
    _guard: ListenerGuard,
    /// Cleared on drop, under its lock: a call that read the rate before the
    /// listener went cannot report it after, onto whatever device replaced
    /// this one.
    live: std::sync::Arc<parking_lot::Mutex<bool>>,
}

impl Drop for RateWatch {
    fn drop(&mut self) {
        *self.live.lock() = false;
    }
}

/// Subscribe to nominal sample rate changes on a device.
pub fn watch_device_sample_rate(
    device_id: AudioDeviceID,
    on_change: Box<dyn Fn(f64) + Send + Sync>,
) -> Result<RateWatch> {
    let live = std::sync::Arc::new(parking_lot::Mutex::new(true));
    let guard = ListenerGuard::add(
        device_id,
        std::sync::Arc::new({
            let live = live.clone();
            move || {
                // Read before the lock: a HAL call is never made holding it.
                let Ok(rate) = get_device_sample_rate(device_id) else {
                    return;
                };
                let live = live.lock();
                if *live {
                    on_change(rate);
                }
            }
        }),
    )?;
    Ok(RateWatch {
        _guard: guard,
        live,
    })
}
