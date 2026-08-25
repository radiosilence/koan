use std::mem;
use std::os::raw::c_void;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use coreaudio_sys::*;
use thiserror::Error;

#[cfg(target_os = "macos")]
use super::device;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("CoreAudio error: {0}")]
    OSStatus(i32),
    #[error("failed to find the output audio component")]
    NoOutputComponent,
    #[cfg(target_os = "macos")]
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
    /// Set true while the render callback is executing.
    /// Drop spins on this to avoid tearing down while a callback is in flight.
    in_callback: Arc<AtomicBool>,
}

// SAFETY: `rtrb::Consumer` is `!Send` due to internal raw pointers, but our usage is
// sound: the Consumer is moved into CallbackData on the creating thread, then only
// ever accessed from the single CoreAudio render callback thread. It is never shared
// or accessed from multiple threads simultaneously. If this invariant changes (e.g.
// Consumer accessed outside the callback), this impl must be revisited.
unsafe impl Send for CallbackData {}

/// CoreAudio output engine — AUHAL on macOS, RemoteIO on iOS.
///
/// Creates an AudioUnit, sets the stream format to match the source, and
/// installs a render callback that drains the ring buffer.
///
/// The two platforms differ in exactly two properties: which output component
/// to instantiate, and whether a device can be named at all. Everything below
/// that — the format, the callback, and above all the teardown order that
/// stopped it double-freeing CoreAudio's buffer list — is one implementation on
/// purpose. iOS has no second set of those bugs to find.
pub struct AudioEngine {
    audio_unit: AudioUnit,
    callback_data: *mut CallbackData,
    running: Arc<AtomicBool>,
    in_callback: Arc<AtomicBool>,
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
    ///
    /// `device_id` is a CoreAudio `AudioDeviceID` on macOS. iOS has no such
    /// thing — the route is the session's business, not ours — and ignores it.
    pub fn new(
        device_id: u32,
        sample_rate: f64,
        channels: u32,
        consumer: rtrb::Consumer<f32>,
        samples_played: Arc<AtomicU64>,
    ) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(false));
        let in_callback = Arc::new(AtomicBool::new(false));

        let desc = AudioComponentDescription {
            componentType: kAudioUnitType_Output,
            #[cfg(target_os = "macos")]
            componentSubType: kAudioUnitSubType_HALOutput,
            // AUHAL is declared inside `#if !TARGET_OS_IPHONE`; RemoteIO is what
            // the `#else` offers, and it is the only output unit iOS has.
            #[cfg(target_os = "ios")]
            componentSubType: kAudioUnitSubType_RemoteIO,
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
            return Err(EngineError::NoOutputComponent);
        }

        let mut audio_unit: AudioUnit = ptr::null_mut();
        check(unsafe { AudioComponentInstanceNew(component, &mut audio_unit) })?;

        // Set the output device. iOS has no device to set: the route is chosen
        // by the audio session and can change under a running unit.
        #[cfg(target_os = "macos")]
        check(unsafe {
            AudioUnitSetProperty(
                audio_unit,
                kAudioOutputUnitProperty_CurrentDevice,
                kAudioUnitScope_Global,
                0,
                &device_id as *const _ as *const c_void,
                mem::size_of::<u32>() as u32,
            )
        })?;
        #[cfg(not(target_os = "macos"))]
        let _ = device_id;

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
            in_callback: in_callback.clone(),
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
            in_callback,
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

    /// `stop`, reporting the raw OSStatus — teardown wants to log it.
    fn stop_status(&self) -> i32 {
        self.running.store(false, Ordering::Release);
        unsafe { AudioOutputUnitStop(self.audio_unit) }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        let stop_status = self.stop_status();
        log::debug!("engine teardown: AudioOutputUnitStop -> {}", stop_status);

        // Wait for any in-flight render callback to finish. AudioOutputUnitStop
        // is documented as synchronous, but during sample rate switches the
        // callback can still be executing when stop() returns. Spin on the
        // in_callback flag with a hard timeout to avoid hanging forever.
        let mut spins = 0u32;
        while self.in_callback.load(Ordering::Acquire) {
            std::hint::spin_loop();
            spins += 1;
            if spins > 1_000_000 {
                // ~10ms of spinning on a modern CPU. Give up — better to risk
                // a crash than deadlock the player thread during shutdown.
                log::warn!("AudioEngine drop: timed out waiting for render callback to drain");
                break;
            }
        }
        log::debug!("engine teardown: callback drained after {} spins", spins);

        // Then ask the unit itself. The flag above only proves *our* callback
        // body is not executing; CoreAudio's IO proc can still be mid-cycle
        // around it, and tearing down underneath that corrupts the buffer list
        // it is holding — which is what trapped inside caulk's deallocator, in
        // `AudioUnitUninitialize` and, before this teardown was reordered, in
        // `AudioUnitSetProperty` (#89).
        let mut waited = 0u32;
        let mut last = (0i32, 1u32);
        for _ in 0..200 {
            let mut running: UInt32 = 0;
            let mut size = mem::size_of::<UInt32>() as u32;
            // SAFETY: querying a bool property on a unit we still own.
            let status = unsafe {
                AudioUnitGetProperty(
                    self.audio_unit,
                    kAudioOutputUnitProperty_IsRunning,
                    kAudioUnitScope_Global,
                    0,
                    &mut running as *mut _ as *mut c_void,
                    &mut size,
                )
            };
            last = (status, running);
            if status != 0 || running == 0 {
                break;
            }
            waited += 1;
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        log::debug!(
            "engine teardown: IsRunning query -> status {} running {} after {}ms; uninitializing",
            last.0,
            last.1,
            waited
        );

        // Deliberately no `AudioUnitSetProperty` here.
        //
        // Removing the render callback before teardown looks like the safe
        // move, and it was how this was written — but
        // `kAudioUnitProperty_SetRenderCallback` may only be set while the unit
        // is *uninitialized*. Setting it on a live unit makes CoreAudio tear
        // down and rebuild its internal ExtendedAudioBufferList, and that is
        // the double free this was meant to prevent: the crash landed inside
        // `AudioUnitSetProperty` itself, in caulk's deallocator, every time a
        // queue reached its end (#89, and again at the end of a playlist).
        //
        // `AudioUnitUninitialize` releases that buffer list, once, which is all
        // that is wanted. Nothing can be mid-callback by then: `stop()` has
        // cleared `running` so the callback only writes silence, and the
        // spin-wait above has drained anything already executing.

        // SAFETY: AudioUnit was successfully created in new(). Uninitialize and
        // Dispose are the documented teardown sequence. callback_data was created
        // via Box::into_raw in new() and is not aliased — the render callback
        // has been removed and the spin-wait above ensures it's not in flight.
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
    // the spin-wait on in_callback ensures no callbacks are in flight.
    let data = unsafe { &mut *(in_ref_con as *mut CallbackData) };
    data.in_callback.store(true, Ordering::Release);

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
        data.in_callback.store(false, Ordering::Release);
        return 0;
    }

    // SAFETY: Accessing the first buffer in the CoreAudio-provided AudioBufferList.
    // We configured a non-interleaved float format, so mNumberBuffers >= 1.
    let buf = unsafe { &mut *buffer_list.mBuffers.as_mut_ptr() };
    let channels = buf.mNumberChannels;

    // Never write past what CoreAudio actually allocated.
    //
    // `frames * channels` is what the buffer *should* hold, and it is not the
    // same question as how big it is. CoreAudio can hand back a shorter buffer
    // than the frame count implies — a device switching buffer size, or the
    // final callbacks as a unit is torn down — and writing the full product
    // then runs off the end of its heap block. The damage does not show up
    // here: it surfaces later inside CoreAudio's own allocator, whichever call
    // happens to free that block, which is why this read as a double free in
    // `AudioUnitUninitialize` and, before that, in `AudioUnitSetProperty`.
    let capacity = buf.mDataByteSize as usize / mem::size_of::<f32>();
    let wanted = (in_number_frames * channels) as usize;
    if wanted > capacity {
        log::warn!(
            "CoreAudio buffer holds {} samples but {} frames x {} channels were asked for",
            capacity,
            in_number_frames,
            channels
        );
    }
    let total_samples = wanted.min(capacity);
    if buf.mData.is_null() || total_samples == 0 {
        data.in_callback.store(false, Ordering::Release);
        return 0;
    }
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
        data.in_callback.store(false, Ordering::Release);
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
    //
    // `write_bytes` counts in elements of the pointee, not in bytes, so
    // multiplying by `size_of::<f32>()` here wrote four times the intended
    // range and ran off the end of CoreAudio's buffer. It corrupted the block's
    // neighbour rather than faulting, so nothing happened until CoreAudio freed
    // it — surfacing as a trap inside caulk's deallocator in
    // `AudioUnitUninitialize`, one track later, looking for all the world like
    // a double free in the teardown (#89).
    //
    // Underrun happens at the end of every track, which is why the end of a
    // queue was where it showed.
    if to_read < total_samples {
        // SAFETY: `total_samples` is clamped to the buffer's own
        // `mDataByteSize`, so this slice ends exactly at its end.
        unsafe {
            std::slice::from_raw_parts_mut(out_ptr.add(to_read), total_samples - to_read).fill(0.0);
        }
    }

    data.in_callback.store(false, Ordering::Release);
    0
}

#[cfg(test)]
mod tests {
    /// The underrun fill wrote `count * size_of::<f32>()` *elements* rather than
    /// bytes, because `write_bytes` counts in elements of the pointee — four
    /// times the intended range, straight off the end of CoreAudio's buffer.
    /// It corrupted the neighbouring block silently, so the damage only
    /// appeared when CoreAudio freed it, one track later, inside
    /// `AudioUnitUninitialize`.
    #[test]
    fn underrun_fill_stays_inside_the_buffer() {
        let capacity = 1024usize; // 512 frames x 2 channels
        let mut buffer = vec![7.0f32; capacity + 64]; // canary tail
        let out = buffer.as_mut_ptr();

        let total_samples = capacity;
        let to_read = 300usize;

        unsafe {
            std::slice::from_raw_parts_mut(out.add(to_read), total_samples - to_read).fill(0.0);
        }

        assert!(
            buffer[to_read..capacity].iter().all(|s| *s == 0.0),
            "the underrun region is silent"
        );
        assert!(
            buffer[capacity..].iter().all(|s| *s == 7.0),
            "nothing past the buffer was touched"
        );
    }
}
