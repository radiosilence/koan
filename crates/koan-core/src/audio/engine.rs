use std::mem;
use std::os::raw::c_void;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use coreaudio_sys::*;
use thiserror::Error;

use super::device;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("CoreAudio error: {0}")]
    OSStatus(i32),
    #[error("failed to find HAL output component")]
    NoHalOutput,
    #[error("device error: {0}")]
    Device(#[from] device::DeviceError),
}

type Result<T> = std::result::Result<T, EngineError>;

fn check(status: OSStatus) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(EngineError::OSStatus(status))
    }
}

/// Data shared with the render callback. Must be Send (goes to the audio thread).
struct CallbackData {
    consumer: rtrb::Consumer<f32>,
    running: Arc<AtomicBool>,
    /// Cumulative samples played — incremented by the render callback,
    /// read by the UI to derive current track + position.
    samples_played: Arc<AtomicU64>,
}

// SAFETY: `rtrb::Consumer` is `!Send` due to internal raw pointers, but our usage is
// sound: the Consumer is moved into CallbackData on the creating thread, then only
// ever accessed from the single CoreAudio render callback thread. It is never shared
// or accessed from multiple threads simultaneously. If this invariant changes (e.g.
// Consumer accessed outside the callback), this impl must be revisited.
unsafe impl Send for CallbackData {}

/// CoreAudio AUHAL output engine.
///
/// Creates an AudioUnit targeting a specific device, sets the stream format
/// to match the source, and installs a render callback that drains the ring buffer.
pub struct AudioEngine {
    audio_unit: AudioUnit,
    callback_data: *mut CallbackData,
    running: Arc<AtomicBool>,
}

// SAFETY: AudioEngine contains an AudioUnit (opaque C pointer) and a *mut CallbackData.
// The engine is created on one thread, moved to the player thread, then only used for
// start/stop/drop — all of which are sequentially called from one thread at a time.
// The AudioUnit and callback_data are accessed by the CoreAudio RT thread only through
// the installed render callback, which is removed before drop. AudioEngine is not Clone
// and not shared — it has a single owner at all times.
unsafe impl Send for AudioEngine {}

impl AudioEngine {
    /// Create an engine targeting the given device, expecting the given format.
    pub fn new(
        device_id: AudioDeviceID,
        sample_rate: f64,
        channels: u32,
        consumer: rtrb::Consumer<f32>,
        samples_played: Arc<AtomicU64>,
    ) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(false));

        let desc = AudioComponentDescription {
            componentType: kAudioUnitType_Output,
            componentSubType: kAudioUnitSubType_HALOutput,
            componentManufacturer: kAudioUnitManufacturer_Apple,
            componentFlags: 0,
            componentFlagsMask: 0,
        };

        // SAFETY: All CoreAudio FFI calls below pass stack-allocated structs with
        // correct sizes via mem::size_of. Pointers are valid for the duration of each
        // call. Return values are checked via check(). The AudioUnit is created, configured,
        // and initialized in sequence — no concurrent access is possible during setup.

        let component = unsafe { AudioComponentFindNext(ptr::null_mut(), &desc) };
        if component.is_null() {
            return Err(EngineError::NoHalOutput);
        }

        let mut audio_unit: AudioUnit = ptr::null_mut();
        check(unsafe { AudioComponentInstanceNew(component, &mut audio_unit) })?;

        // Set the output device.
        check(unsafe {
            AudioUnitSetProperty(
                audio_unit,
                kAudioOutputUnitProperty_CurrentDevice,
                kAudioUnitScope_Global,
                0,
                &device_id as *const _ as *const c_void,
                mem::size_of::<AudioDeviceID>() as u32,
            )
        })?;

        // Set stream format on the input scope of the output element.
        // This tells the AudioUnit what format we'll provide in the render callback.
        let bytes_per_sample = mem::size_of::<f32>() as u32;
        let asbd = AudioStreamBasicDescription {
            mSampleRate: sample_rate,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
            mBytesPerPacket: bytes_per_sample * channels,
            mFramesPerPacket: 1,
            mBytesPerFrame: bytes_per_sample * channels,
            mChannelsPerFrame: channels,
            mBitsPerChannel: 32,
            mReserved: 0,
        };

        check(unsafe {
            AudioUnitSetProperty(
                audio_unit,
                kAudioUnitProperty_StreamFormat,
                kAudioUnitScope_Input,
                0,
                &asbd as *const _ as *const c_void,
                mem::size_of::<AudioStreamBasicDescription>() as u32,
            )
        })?;

        // Allocate callback data on the heap — the render callback gets a raw pointer to it.
        let callback_data = Box::into_raw(Box::new(CallbackData {
            consumer,
            running: running.clone(),
            samples_played,
        }));

        let render_cb = AURenderCallbackStruct {
            inputProc: Some(render_callback),
            inputProcRefCon: callback_data as *mut c_void,
        };

        check(unsafe {
            AudioUnitSetProperty(
                audio_unit,
                kAudioUnitProperty_SetRenderCallback,
                kAudioUnitScope_Input,
                0,
                &render_cb as *const _ as *const c_void,
                mem::size_of::<AURenderCallbackStruct>() as u32,
            )
        })?;

        check(unsafe { AudioUnitInitialize(audio_unit) })?;

        Ok(Self {
            audio_unit,
            callback_data,
            running,
        })
    }

    pub fn start(&self) -> Result<()> {
        self.running.store(true, Ordering::Release);
        check(unsafe { AudioOutputUnitStart(self.audio_unit) })
    }

    pub fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::Release);
        check(unsafe { AudioOutputUnitStop(self.audio_unit) })
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        let _ = self.stop();

        // Remove the render callback before tearing down. This ensures no
        // in-flight callback can race with AudioUnitUninitialize, which
        // otherwise crashes in CoreAudio's internal allocator (caulk) —
        // especially during sample rate changes.
        let silent_cb = AURenderCallbackStruct {
            inputProc: None,
            inputProcRefCon: ptr::null_mut(),
        };
        // SAFETY: Removing the callback from a valid AudioUnit. Even if the unit
        // is in a degraded state, setting a null callback is a no-op at worst.
        unsafe {
            AudioUnitSetProperty(
                self.audio_unit,
                kAudioUnitProperty_SetRenderCallback,
                kAudioUnitScope_Input,
                0,
                &silent_cb as *const _ as *const c_void,
                mem::size_of::<AURenderCallbackStruct>() as u32,
            );
        }

        // No explicit wait needed here: AudioOutputUnitStop (called by stop()
        // above) is synchronous — CoreAudio guarantees the render callback has
        // fully returned before it hands control back. The callback removal
        // above is belt-and-suspenders for the (extremely rare) case where
        // stop() returns an error and the unit is in a degraded state.

        // SAFETY: AudioUnit was successfully created in new(). Uninitialize and
        // Dispose are the documented teardown sequence. callback_data was created
        // via Box::into_raw in new() and is not aliased — the render callback
        // has been removed and AudioOutputUnitStop guarantees it's not in flight.
        unsafe {
            AudioUnitUninitialize(self.audio_unit);
            AudioComponentInstanceDispose(self.audio_unit);
            drop(Box::from_raw(self.callback_data));
        }
    }
}

/// Render callback — called on the CoreAudio real-time thread.
///
/// This MUST NOT allocate, lock, or do anything that could block.
/// It drains f32 samples from the rtrb ring buffer into CoreAudio's output buffer.
unsafe extern "C" fn render_callback(
    in_ref_con: *mut c_void,
    _action_flags: *mut AudioUnitRenderActionFlags,
    _timestamp: *const AudioTimeStamp,
    _bus_number: UInt32,
    in_number_frames: UInt32,
    io_data: *mut AudioBufferList,
) -> OSStatus {
    // SAFETY: `in_ref_con` points to a heap-allocated CallbackData created via
    // Box::into_raw in AudioEngine::new. It remains valid for the lifetime of the
    // engine — the callback is removed and the pointer freed only in Drop, after
    // AudioOutputUnitStop guarantees no callbacks are in flight.
    let data = unsafe { &mut *(in_ref_con as *mut CallbackData) };
    // SAFETY: `io_data` is provided by CoreAudio and is valid for the callback's duration.
    let buffer_list = unsafe { &mut *io_data };

    if !data.running.load(Ordering::Acquire) {
        for i in 0..buffer_list.mNumberBuffers as usize {
            let buf = unsafe { &mut *buffer_list.mBuffers.as_mut_ptr().add(i) };
            if !buf.mData.is_null() {
                unsafe {
                    ptr::write_bytes(buf.mData as *mut u8, 0, buf.mDataByteSize as usize);
                }
            }
        }
        return 0;
    }

    // SAFETY: Accessing the first buffer in the CoreAudio-provided AudioBufferList.
    // We configured a non-interleaved float format, so mNumberBuffers >= 1.
    let buf = unsafe { &mut *buffer_list.mBuffers.as_mut_ptr() };
    let channels = buf.mNumberChannels;
    let total_samples = (in_number_frames * channels) as usize;
    if !(buf.mData as usize).is_multiple_of(mem::align_of::<f32>()) {
        log::error!("CoreAudio buffer not aligned for f32");
        // Fill silence rather than risking UB from an unaligned cast.
        for i in 0..buffer_list.mNumberBuffers as usize {
            let b = unsafe { &mut *buffer_list.mBuffers.as_mut_ptr().add(i) };
            if !b.mData.is_null() {
                unsafe {
                    ptr::write_bytes(b.mData as *mut u8, 0, b.mDataByteSize as usize);
                }
            }
        }
        return 0;
    }
    let out_ptr = buf.mData as *mut f32;

    let available = data.consumer.slots();
    let to_read = available.min(total_samples);

    if to_read > 0
        && let Ok(chunk) = data.consumer.read_chunk(to_read)
    {
        let (first, second) = chunk.as_slices();
        let ring_total = first.len() + second.len();
        let copy_total = ring_total.min(total_samples);
        let first_copy = first.len().min(copy_total);
        let second_copy = copy_total.saturating_sub(first_copy).min(second.len());
        unsafe {
            ptr::copy_nonoverlapping(first.as_ptr(), out_ptr, first_copy);
            if second_copy > 0 {
                ptr::copy_nonoverlapping(second.as_ptr(), out_ptr.add(first_copy), second_copy);
            }
        }
        chunk.commit_all();
        data.samples_played
            .fetch_add(copy_total as u64, Ordering::AcqRel);
    }

    // Zero remaining frames on underrun — silence > glitches.
    if to_read < total_samples {
        unsafe {
            ptr::write_bytes(
                out_ptr.add(to_read),
                0,
                (total_samples - to_read) * mem::size_of::<f32>(),
            );
        }
    }

    0
}
