//! Android output: an AAudio stream, bit-perfect to a USB DAC where Android
//! allows it (see `bit_perfect`), through the system mixer otherwise.
//!
//! The one device is the route Android has chosen. A stream follows it only
//! until the device goes, when AAudio disconnects it; the app rebuilds the
//! output on a route change (`restart_output`).

use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

use parking_lot::Mutex;

use super::backend::{AudioBackend, AudioEngineHandle, BackendError, DeviceInfo};
use super::bit_perfect::{self, Encoding, MixerFormat};
use super::fade::{FadeControl, Fader};

/// The rate Android's mixer usually runs at, for when no app has said.
const USUAL_MIXER_RATE: u32 = 48000;

#[allow(non_camel_case_types)]
type aaudio_result_t = i32;

#[repr(C)]
struct AAudioStreamBuilder {
    _private: [u8; 0],
}

#[repr(C)]
struct AAudioStream {
    _private: [u8; 0],
}

type DataCallback = unsafe extern "C" fn(*mut AAudioStream, *mut c_void, *mut c_void, i32) -> i32;
type ErrorCallback = unsafe extern "C" fn(*mut AAudioStream, *mut c_void, aaudio_result_t);

const AAUDIO_OK: aaudio_result_t = 0;
const AAUDIO_ERROR_DISCONNECTED: aaudio_result_t = -899;
const AAUDIO_UNSPECIFIED: i32 = 0;
const AAUDIO_SHARING_MODE_SHARED: i32 = 1;
const AAUDIO_PERFORMANCE_MODE_NONE: i32 = 10;
const AAUDIO_USAGE_MEDIA: i32 = 1;
const AAUDIO_CONTENT_TYPE_MUSIC: i32 = 2;
const AAUDIO_CALLBACK_RESULT_CONTINUE: i32 = 0;

#[link(name = "aaudio")]
unsafe extern "C" {
    fn AAudio_createStreamBuilder(builder: *mut *mut AAudioStreamBuilder) -> aaudio_result_t;
    fn AAudio_convertResultToText(result: aaudio_result_t) -> *const c_char;
    fn AAudioStreamBuilder_setDeviceId(builder: *mut AAudioStreamBuilder, id: i32);
    fn AAudioStreamBuilder_setSampleRate(builder: *mut AAudioStreamBuilder, rate: i32);
    fn AAudioStreamBuilder_setChannelCount(builder: *mut AAudioStreamBuilder, channels: i32);
    fn AAudioStreamBuilder_setFormat(builder: *mut AAudioStreamBuilder, format: i32);
    fn AAudioStreamBuilder_setSharingMode(builder: *mut AAudioStreamBuilder, mode: i32);
    fn AAudioStreamBuilder_setPerformanceMode(builder: *mut AAudioStreamBuilder, mode: i32);
    fn AAudioStreamBuilder_setUsage(builder: *mut AAudioStreamBuilder, usage: i32);
    fn AAudioStreamBuilder_setContentType(builder: *mut AAudioStreamBuilder, content: i32);
    fn AAudioStreamBuilder_setDataCallback(
        builder: *mut AAudioStreamBuilder,
        callback: DataCallback,
        user: *mut c_void,
    );
    fn AAudioStreamBuilder_setErrorCallback(
        builder: *mut AAudioStreamBuilder,
        callback: ErrorCallback,
        user: *mut c_void,
    );
    fn AAudioStreamBuilder_openStream(
        builder: *mut AAudioStreamBuilder,
        stream: *mut *mut AAudioStream,
    ) -> aaudio_result_t;
    fn AAudioStreamBuilder_delete(builder: *mut AAudioStreamBuilder) -> aaudio_result_t;
    fn AAudioStream_requestStart(stream: *mut AAudioStream) -> aaudio_result_t;
    fn AAudioStream_requestStop(stream: *mut AAudioStream) -> aaudio_result_t;
    fn AAudioStream_close(stream: *mut AAudioStream) -> aaudio_result_t;
    fn AAudioStream_getBufferCapacityInFrames(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getXRunCount(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getSampleRate(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getFormat(stream: *mut AAudioStream) -> i32;
}

fn result_text(result: aaudio_result_t) -> String {
    // SAFETY: AAudio returns a static, NUL-terminated string for any result.
    unsafe { CStr::from_ptr(AAudio_convertResultToText(result)) }
        .to_string_lossy()
        .into_owned()
}

fn check(result: aaudio_result_t, what: &str) -> Result<(), BackendError> {
    if result == AAUDIO_OK {
        Ok(())
    } else {
        Err(BackendError::StreamCreation(format!(
            "{what}: {}",
            result_text(result)
        )))
    }
}

/// The output, and the bit-perfect format it holds, if any.
#[derive(Default)]
pub struct AndroidAudioBackend {
    held: Mutex<Option<MixerFormat>>,
    disconnect: Mutex<Option<Disconnect>>,
}

impl AndroidAudioBackend {
    fn device() -> DeviceInfo {
        DeviceInfo {
            name: "Android".into(),
            sample_rates: Vec::new(),
            platform_id: 0,
            kind: Default::default(),
        }
    }

    fn mixer_rate() -> u32 {
        bit_perfect::host().map_or(USUAL_MIXER_RATE, |h| h.mixer_rate())
    }
}

impl AudioBackend for AndroidAudioBackend {
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        Ok(vec![Self::device()])
    }

    fn default_device(&self) -> Result<DeviceInfo, BackendError> {
        Ok(Self::device())
    }

    fn supported_sample_rates(&self, _device: &DeviceInfo) -> Result<Vec<f64>, BackendError> {
        Ok(Vec::new())
    }

    fn get_device_sample_rate(&self, _device: &DeviceInfo) -> Result<f64, BackendError> {
        Ok(self.held.lock().map_or_else(Self::mixer_rate, |f| f.rate) as f64)
    }

    fn set_device_sample_rate(&self, device: &DeviceInfo, rate: f64) -> Result<f64, BackendError> {
        self.prepare_output(device, 0.0, rate, 2, None)
    }

    /// Hold the USB DAC bit-perfect at the stream's format if it offers it,
    /// and give the mixer back otherwise. Answers the rate the device runs
    /// at: the stream's when held, the mixer's when not.
    fn prepare_output(
        &self,
        _device: &DeviceInfo,
        _current: f64,
        rate: f64,
        channels: u16,
        bit_depth: Option<u16>,
    ) -> Result<f64, BackendError> {
        let mut held = self.held.lock();
        let Some(host) = bit_perfect::host() else {
            return Ok(USUAL_MIXER_RATE as f64);
        };
        let wanted = bit_perfect::choose(&host.offers(), rate as u32, channels, bit_depth);
        if wanted.is_some() && wanted == *held {
            return Ok(rate);
        }
        if let Some(format) = wanted
            && host.hold(format)
        {
            log::info!(
                "android output: bit-perfect, {} Hz {:?} on USB device {}",
                format.rate,
                format.encoding,
                format.device_id
            );
            *held = Some(format);
            return Ok(rate);
        }
        if held.take().is_some() {
            host.release();
        }
        let mixer = host.mixer_rate();
        log::info!(
            "android output: through the mixer at {mixer} Hz ({})",
            if wanted.is_some() {
                "the USB DAC refused bit-perfect"
            } else {
                "no USB DAC offers this format bit-perfect"
            }
        );
        Ok(mixer as f64)
    }

    fn output_mixed(&self) -> Option<bool> {
        Some(self.held.lock().is_none())
    }

    fn output_depth(&self) -> Option<u16> {
        self.held.lock().map(|f| f.encoding.bits())
    }

    fn on_disconnect(&self, rebuild: Arc<dyn Fn() + Send + Sync>) {
        *self.disconnect.lock() = Some(rebuild);
    }

    fn create_engine(
        &self,
        _device: &DeviceInfo,
        sample_rate: f64,
        channels: u32,
        consumer: rtrb::Consumer<f32>,
        samples_played: Arc<AtomicU64>,
    ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
        let fade = FadeControl::for_output();
        let state = Box::into_raw(Box::new(Callback {
            consumer,
            fader: Fader::new(fade.clone(), sample_rate),
            samples_played,
            running: Arc::new(AtomicBool::new(false)),
            channels: channels as usize,
            encoding: Encoding::Float,
            scratch: Vec::new(),
            xruns: Arc::new(AtomicI32::new(0)),
            lead_in: Arc::new(AtomicU64::new(0)),
            pace: Arc::new(Pace::default()),
        }));
        let disconnect = Box::into_raw(Box::new(self.disconnect.lock().clone()));

        let mut held = self.held.lock();
        let mut opened = Err(BackendError::NoDevices);
        if let Some(format) = *held {
            // SAFETY: `state` and `disconnect` outlive any stream opened on them.
            opened = unsafe {
                open(
                    sample_rate,
                    channels,
                    format.encoding,
                    format.device_id,
                    state,
                    disconnect,
                )
            };
            if let Err(e) = &opened {
                // Android took the attributes but will not open a stream to
                // them: play through the mixer rather than not at all.
                log::warn!("android output: bit-perfect stream refused ({e}); through the mixer");
                if let Some(host) = bit_perfect::host() {
                    host.release();
                }
                *held = None;
            }
        }
        if held.is_none() {
            // SAFETY: as above.
            opened = unsafe {
                open(
                    sample_rate,
                    channels,
                    Encoding::Float,
                    AAUDIO_UNSPECIFIED,
                    state,
                    disconnect,
                )
            };
        }
        let stream = match opened {
            Ok(stream) => stream,
            Err(e) => {
                // SAFETY: no stream holds them.
                unsafe {
                    drop(Box::from_raw(state));
                    drop(Box::from_raw(disconnect));
                }
                return Err(e);
            }
        };
        // SAFETY: the stream is open but not started, so no callback runs.
        let callback = unsafe { &mut *state };
        callback.encoding = held.map_or(Encoding::Float, |f| f.encoding);
        // A callback is never asked for more frames than the buffer holds,
        // so this is the most it converts at once.
        let capacity = unsafe { AAudioStream_getBufferCapacityInFrames(stream) }.max(1) as usize;
        callback.scratch = vec![0.0; capacity * channels as usize];
        Ok(Box::new(AndroidEngine {
            rate: sample_rate,
            channels,
            pace: callback.pace.clone(),
            stream,
            state,
            disconnect,
            running: callback.running.clone(),
            fade,
            xruns: callback.xruns.clone(),
            lead_in: callback.lead_in.clone(),
        }))
    }
}

/// Open a stream in `encoding` on `device`, or fail if AAudio would open it in
/// anything else: the callback writes that encoding, and a stream of another
/// would play it as noise.
///
/// # Safety
///
/// `state` and `disconnect` must outlive the stream.
unsafe fn open(
    rate: f64,
    channels: u32,
    encoding: Encoding,
    device: i32,
    state: *mut Callback,
    disconnect: *mut Option<Disconnect>,
) -> Result<*mut AAudioStream, BackendError> {
    let mut builder = std::ptr::null_mut();
    // SAFETY: AAudio's builder and stream calls, on pointers it returned.
    unsafe {
        check(AAudio_createStreamBuilder(&mut builder), "builder")?;
        AAudioStreamBuilder_setDeviceId(builder, device);
        AAudioStreamBuilder_setSampleRate(builder, rate as i32);
        AAudioStreamBuilder_setChannelCount(builder, channels as i32);
        AAudioStreamBuilder_setFormat(builder, encoding.aaudio());
        AAudioStreamBuilder_setSharingMode(builder, AAUDIO_SHARING_MODE_SHARED);
        AAudioStreamBuilder_setPerformanceMode(builder, AAUDIO_PERFORMANCE_MODE_NONE);
        AAudioStreamBuilder_setUsage(builder, AAUDIO_USAGE_MEDIA);
        AAudioStreamBuilder_setContentType(builder, AAUDIO_CONTENT_TYPE_MUSIC);
        AAudioStreamBuilder_setDataCallback(builder, render, state.cast());
        AAudioStreamBuilder_setErrorCallback(builder, stream_error, disconnect.cast());
        let mut stream = std::ptr::null_mut();
        let opened = AAudioStreamBuilder_openStream(builder, &mut stream);
        AAudioStreamBuilder_delete(builder);
        check(opened, "open")?;
        let (got_rate, got_format) = (
            AAudioStream_getSampleRate(stream),
            AAudioStream_getFormat(stream),
        );
        if got_rate != rate as i32 || got_format != encoding.aaudio() {
            AAudioStream_close(stream);
            return Err(BackendError::StreamCreation(format!(
                "asked for {rate} Hz format {}, AAudio offered {got_rate} Hz format {got_format}",
                encoding.aaudio()
            )));
        }
        Ok(stream)
    }
}

/// What the app gave to be told when a stream is disconnected.
type Disconnect = Arc<dyn Fn() + Send + Sync>;

/// What the render callback owns. Allocated before the stream opens and
/// freed after it closes; the callback itself neither allocates nor locks.
struct Callback {
    consumer: rtrb::Consumer<f32>,
    fader: Fader,
    samples_played: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    channels: usize,
    encoding: Encoding,
    /// Samples read from the ring and faded, before conversion.
    scratch: Vec<f32>,
    xruns: Arc<AtomicI32>,
    lead_in: Arc<AtomicU64>,
    /// The rate AAudio asks for frames at, against the monotonic clock: the
    /// first and latest callback's time, and the frames asked for in between.
    pace: Arc<Pace>,
}

/// How fast the stream consumes, measured: what the playhead's prediction
/// assumes is the nominal rate.
#[derive(Default)]
struct Pace {
    first_ns: AtomicU64,
    last_ns: AtomicU64,
    frames: AtomicU64,
    /// The latest callback, for `presentation`: the samples played once it
    /// had run, the ring samples it took, and when it ended. Written as one
    /// under `seq`, odd while a write is in progress.
    seq: AtomicU64,
    played: AtomicU64,
    taken: AtomicU64,
    at_ns: AtomicU64,
}

impl Pace {
    fn note(&self, frames: u64) {
        let now = monotonic_ns();
        if self.first_ns.load(Ordering::Relaxed) == 0 {
            self.first_ns.store(now, Ordering::Relaxed);
        } else {
            self.frames.fetch_add(frames, Ordering::Relaxed);
        }
        self.last_ns.store(now, Ordering::Relaxed);
    }

    /// Record the callback just run. Wait-free: the reader retries instead.
    fn record(&self, played: u64, taken: u64) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq + 1, Ordering::Relaxed);
        std::sync::atomic::fence(Ordering::Release);
        self.played.store(played, Ordering::Relaxed);
        self.taken.store(taken, Ordering::Relaxed);
        self.at_ns.store(monotonic_ns(), Ordering::Relaxed);
        self.seq.store(seq + 2, Ordering::Release);
    }

    /// The latest callback's record, whole.
    fn latest(&self) -> (u64, u64, u64) {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            let record = (
                self.played.load(Ordering::Relaxed),
                self.taken.load(Ordering::Relaxed),
                self.at_ns.load(Ordering::Relaxed),
            );
            std::sync::atomic::fence(Ordering::Acquire);
            if before.is_multiple_of(2) && self.seq.load(Ordering::Relaxed) == before {
                return record;
            }
            std::hint::spin_loop();
        }
    }

    /// Frames a second, once the stream has run long enough to say.
    fn rate(&self) -> Option<f64> {
        let span = self
            .last_ns
            .load(Ordering::Relaxed)
            .saturating_sub(self.first_ns.load(Ordering::Relaxed));
        (span > 1_000_000_000)
            .then(|| self.frames.load(Ordering::Relaxed) as f64 * 1e9 / span as f64)
    }
}

fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid timespec to write into; CLOCK_MONOTONIC always exists.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

unsafe extern "C" fn render(
    stream: *mut AAudioStream,
    user: *mut c_void,
    data: *mut c_void,
    frames: i32,
) -> i32 {
    // SAFETY: `user` is the `Callback` the stream was opened with, touched by
    // this callback alone; `data` holds `frames` frames of the stream's format.
    let cb = unsafe { &mut *user.cast::<Callback>() };
    let channels = cb.channels.max(1);
    let bytes = cb.encoding.bytes();
    let total = frames.max(0) as usize * channels;
    let out = unsafe { std::slice::from_raw_parts_mut(data.cast::<u8>(), total * bytes) };
    cb.pace.note(frames.max(0) as u64);
    cb.xruns.store(
        unsafe { AAudioStream_getXRunCount(stream) },
        Ordering::Relaxed,
    );

    if !cb.running.load(Ordering::Acquire) || cb.leading_in(frames.max(0) as u64) {
        // Every encoding's zero is all-zero bytes.
        out.fill(0);
        cb.pace.record(cb.samples_played.load(Ordering::Relaxed), 0);
        return AAUDIO_CALLBACK_RESULT_CONTINUE;
    }

    let mut done = 0;
    let mut taken = 0;
    while done < total {
        let want = (total - done).min(cb.scratch.len() - cb.scratch.len() % channels);
        if want == 0 {
            break;
        }
        let take = cb.consumer.slots().min(cb.fader.readable(want, channels));
        let take = take - take % channels;
        let scratch = &mut cb.scratch[..want];
        let mut got = 0;
        if take > 0
            && let Ok(chunk) = cb.consumer.read_chunk(take)
        {
            let (first, second) = chunk.as_slices();
            scratch[..first.len()].copy_from_slice(first);
            scratch[first.len()..take].copy_from_slice(second);
            chunk.commit_all();
            cb.samples_played.fetch_add(take as u64, Ordering::AcqRel);
            cb.fader.apply(&mut scratch[..take], channels);
            got = take;
            taken += take;
        }
        // Silence past what the ring had: an underrun, or a fade's end.
        scratch[got..].fill(0.0);
        bit_perfect::encode(
            scratch,
            cb.encoding,
            &mut out[done * bytes..(done + want) * bytes],
        );
        done += want;
        if got < want {
            break;
        }
    }
    out[done * bytes..].fill(0);
    cb.pace
        .record(cb.samples_played.load(Ordering::Relaxed), taken as u64);
    AAUDIO_CALLBACK_RESULT_CONTINUE
}

impl Callback {
    /// Whether this callback plays lead-in silence: nothing read from the
    /// ring and nothing counted as played, so the playhead waits with the
    /// device. Taken with a compare-exchange, so a `fade_in` clearing it in
    /// between is not overwritten with what was left.
    fn leading_in(&mut self, frames: u64) -> bool {
        let mut left = self.lead_in.load(Ordering::Relaxed);
        while left > 0 {
            match self.lead_in.compare_exchange_weak(
                left,
                left.saturating_sub(frames),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.fader.hold();
                    return true;
                }
                Err(now) => left = now,
            }
        }
        false
    }
}

/// AAudio's stream errors, on a thread of its own rather than the render
/// callback's. A disconnected stream never plays again: the device went, or
/// the audio server restarted, which disconnects every stream at once. The
/// player is asked to rebuild the output; closing the stream here would
/// deadlock AAudio.
unsafe extern "C" fn stream_error(_: *mut AAudioStream, user: *mut c_void, error: aaudio_result_t) {
    log::warn!("android output: stream error: {}", result_text(error));
    // SAFETY: the `Option<Disconnect>` the stream was opened with, which
    // outlives it and is not written while it is open.
    let disconnect = unsafe { &*user.cast::<Option<Disconnect>>() };
    if error == AAUDIO_ERROR_DISCONNECTED
        && let Some(rebuild) = disconnect
    {
        rebuild();
    }
}

struct AndroidEngine {
    rate: f64,
    channels: u32,
    pace: Arc<Pace>,
    stream: *mut AAudioStream,
    state: *mut Callback,
    disconnect: *mut Option<Disconnect>,
    running: Arc<AtomicBool>,
    fade: Arc<FadeControl>,
    xruns: Arc<AtomicI32>,
    lead_in: Arc<AtomicU64>,
}

// SAFETY: AAudio's stream calls may be made from any thread; the callback
// state is reached only through the stream until `drop` closes it.
unsafe impl Send for AndroidEngine {}

impl Drop for AndroidEngine {
    fn drop(&mut self) {
        // SAFETY: stopping first, as Oboe does: on older releases a close
        // could race a callback still in flight. Closing ends the callbacks,
        // after which nothing else holds `state` or `disconnect`.
        unsafe {
            AAudioStream_requestStop(self.stream);
            AAudioStream_close(self.stream);
            drop(Box::from_raw(self.state));
            drop(Box::from_raw(self.disconnect));
        }
        let xruns = self.xruns.load(Ordering::Relaxed);
        if xruns > 0 {
            log::warn!("android output: {xruns} underruns");
        }
        if let Some(rate) = self.pace.rate() {
            log::info!(
                "android output: consumed {rate:.0} frames/s against {} nominal ({:+.3}%)",
                self.rate,
                (rate / self.rate - 1.0) * 100.0
            );
        }
    }
}

impl AudioEngineHandle for AndroidEngine {
    fn start(&self) -> Result<(), BackendError> {
        self.running.store(true, Ordering::Release);
        // SAFETY: a stream this engine opened and has not closed.
        check(unsafe { AAudioStream_requestStart(self.stream) }, "start")
    }

    fn stop(&self) -> Result<(), BackendError> {
        self.running.store(false, Ordering::Release);
        // SAFETY: as above.
        check(unsafe { AAudioStream_requestStop(self.stream) }, "stop")
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn fade_out(&self) {
        self.fade.fade_out();
    }

    fn fade_out_quickly(&self) {
        self.fade.fade_out_quickly();
    }

    fn period(&self) -> std::time::Duration {
        self.fade.period()
    }

    fn set_sleep_gain(&self, gain: f32, snap: bool) {
        self.fade.set_sleep_gain(gain, snap);
    }

    fn fade_in(&self) -> Result<(), BackendError> {
        // A resume comes long after any rate switch the lead-in was covering.
        self.lead_in.store(0, Ordering::Relaxed);
        if self.is_running() {
            self.fade.fade_in(false);
            return Ok(());
        }
        self.fade.fade_in(true);
        self.start()
    }

    fn fade_in_quickly(&self) -> Result<(), BackendError> {
        if self.is_running() {
            self.fade.fade_in_quickly(false);
            return Ok(());
        }
        self.fade.fade_in_quickly(true);
        self.start()
    }

    fn is_silent(&self) -> bool {
        self.fade.is_silent()
    }

    fn lead_in(&self, frames: u64) {
        self.lead_in.store(frames, Ordering::Relaxed);
    }

    /// AAudio takes a burst at a time through the mixer, 2006 frames or so:
    /// 45 ms at 44.1 kHz, more than a client's tolerance for where the
    /// playhead is. Between callbacks, the samples the last one took are
    /// counted out at the stream's rate rather than all at once.
    fn presentation(&self) -> Option<super::backend::Presentation> {
        let pace = self.pace.clone();
        let per_ns = self.rate * self.channels as f64 / 1e9;
        Some(Arc::new(move || {
            let (played, taken, at) = pace.latest();
            let along = ((monotonic_ns().saturating_sub(at) as f64 * per_ns) as u64).min(taken);
            played.saturating_sub(taken) + along
        }))
    }
}
