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

// SAFETY: Consumer is only ever accessed from the CoreAudio render callback thread
// (single-consumer). The raw pointer prevents auto-Send but the access pattern is safe.
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

// SAFETY: AudioEngine is created on one thread and may be moved to the player thread
// before start() is called. After start(), the AudioUnit and callback_data pointer are
// only accessed from the CoreAudio render thread via the installed callback. The engine
// itself is only used for start/stop/drop which are safe across threads.
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
        self.running.store(true, Ordering::Relaxed);
        check(unsafe { AudioOutputUnitStart(self.audio_unit) })
    }

    pub fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::Relaxed);
        check(unsafe { AudioOutputUnitStop(self.audio_unit) })
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
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

        // Brief yield to let any in-flight render callback on the RT thread
        // finish before we tear down the unit and free callback memory.
        std::thread::yield_now();

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
    let data = unsafe { &mut *(in_ref_con as *mut CallbackData) };
    let buffer_list = unsafe { &mut *io_data };

    if !data.running.load(Ordering::Relaxed) {
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

    let buf = unsafe { &mut *buffer_list.mBuffers.as_mut_ptr() };
    let channels = buf.mNumberChannels;
    let total_samples = (in_number_frames * channels) as usize;
    let out_ptr = buf.mData as *mut f32;

    let available = data.consumer.slots();
    let to_read = available.min(total_samples);

    if to_read > 0
        && let Ok(chunk) = data.consumer.read_chunk(to_read)
    {
        let (first, second) = chunk.as_slices();
        unsafe {
            ptr::copy_nonoverlapping(first.as_ptr(), out_ptr, first.len());
            if !second.is_empty() {
                ptr::copy_nonoverlapping(second.as_ptr(), out_ptr.add(first.len()), second.len());
            }
        }
        chunk.commit_all();
        data.samples_played
            .fetch_add(to_read as u64, Ordering::Relaxed);
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
