//! The slice of AudioToolbox the output engine uses, declared by hand for tvOS.
//!
//! `coreaudio-sys` generates its bindings in a build script that knows macOS
//! and iOS and panics for any other target. RemoteIO is the same C interface on
//! tvOS as on iOS, so the engine needs only these names, laid out as the SDK
//! headers lay them out.
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]

use std::os::raw::c_void;

pub type OSStatus = i32;
pub type UInt32 = u32;
pub type AudioUnitRenderActionFlags = u32;

#[repr(C)]
pub struct OpaqueAudioComponent {
    _private: [u8; 0],
}
pub type AudioComponent = *mut OpaqueAudioComponent;

#[repr(C)]
pub struct ComponentInstanceRecord {
    _private: [u8; 0],
}
pub type AudioComponentInstance = *mut ComponentInstanceRecord;
pub type AudioUnit = AudioComponentInstance;

/// Only ever passed through to the render callback by pointer.
#[repr(C)]
pub struct AudioTimeStamp {
    _private: [u8; 0],
}

#[repr(C)]
pub struct AudioComponentDescription {
    pub componentType: u32,
    pub componentSubType: u32,
    pub componentManufacturer: u32,
    pub componentFlags: u32,
    pub componentFlagsMask: u32,
}

#[repr(C)]
pub struct AudioStreamBasicDescription {
    pub mSampleRate: f64,
    pub mFormatID: u32,
    pub mFormatFlags: u32,
    pub mBytesPerPacket: u32,
    pub mFramesPerPacket: u32,
    pub mBytesPerFrame: u32,
    pub mChannelsPerFrame: u32,
    pub mBitsPerChannel: u32,
    pub mReserved: u32,
}

#[repr(C)]
pub struct AudioBuffer {
    pub mNumberChannels: u32,
    pub mDataByteSize: u32,
    pub mData: *mut c_void,
}

#[repr(C)]
pub struct AudioBufferList {
    pub mNumberBuffers: u32,
    pub mBuffers: [AudioBuffer; 1],
}

pub type AURenderCallback = Option<
    unsafe extern "C" fn(
        inRefCon: *mut c_void,
        ioActionFlags: *mut AudioUnitRenderActionFlags,
        inTimeStamp: *const AudioTimeStamp,
        inBusNumber: u32,
        inNumberFrames: u32,
        ioData: *mut AudioBufferList,
    ) -> OSStatus,
>;

#[repr(C)]
pub struct AURenderCallbackStruct {
    pub inputProc: AURenderCallback,
    pub inputProcRefCon: *mut c_void,
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

pub const kAudioUnitType_Output: u32 = fourcc(b"auou");
pub const kAudioUnitSubType_RemoteIO: u32 = fourcc(b"rioc");
pub const kAudioUnitManufacturer_Apple: u32 = fourcc(b"appl");
pub const kAudioFormatLinearPCM: u32 = fourcc(b"lpcm");
pub const kAudioFormatFlagIsFloat: u32 = 1 << 0;
pub const kAudioFormatFlagIsPacked: u32 = 1 << 3;
pub const kAudioUnitProperty_StreamFormat: u32 = 8;
pub const kAudioUnitProperty_SetRenderCallback: u32 = 23;
pub const kAudioOutputUnitProperty_IsRunning: u32 = 2001;
pub const kAudioUnitScope_Global: u32 = 0;
pub const kAudioUnitScope_Input: u32 = 1;

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    pub fn AudioComponentFindNext(
        inComponent: AudioComponent,
        inDesc: *const AudioComponentDescription,
    ) -> AudioComponent;
    pub fn AudioComponentInstanceNew(
        inComponent: AudioComponent,
        outInstance: *mut AudioComponentInstance,
    ) -> OSStatus;
    pub fn AudioComponentInstanceDispose(inInstance: AudioComponentInstance) -> OSStatus;
    pub fn AudioUnitInitialize(inUnit: AudioUnit) -> OSStatus;
    pub fn AudioUnitUninitialize(inUnit: AudioUnit) -> OSStatus;
    pub fn AudioUnitSetProperty(
        inUnit: AudioUnit,
        inID: u32,
        inScope: u32,
        inElement: u32,
        inData: *const c_void,
        inDataSize: u32,
    ) -> OSStatus;
    pub fn AudioUnitGetProperty(
        inUnit: AudioUnit,
        inID: u32,
        inScope: u32,
        inElement: u32,
        outData: *mut c_void,
        ioDataSize: *mut u32,
    ) -> OSStatus;
    pub fn AudioOutputUnitStart(ci: AudioUnit) -> OSStatus;
    pub fn AudioOutputUnitStop(ci: AudioUnit) -> OSStatus;
}
