use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use crossbeam_channel::{Receiver, Sender};
use ndk::audio::{
    AudioCallbackResult, AudioContentType, AudioDirection, AudioError, AudioFormat,
    AudioPerformanceMode, AudioSharingMode, AudioStream, AudioStreamBuilder, AudioUsage,
};
use parking_lot::Mutex;

use super::android_policy::{
    self as policy, Attempt, Granted, Output, REOPEN_DELAYS, Sharing, StreamFault, render,
};
use super::backend::{AudioBackend, AudioEngineHandle, BackendError, DeviceInfo, SampleRateWatch};
use super::fade::{FadeControl, Fader};

/// Android output, through AAudio.
///
/// The decode pipeline is the same one every platform runs: samples at the
/// source's rate and channel count arrive in the rtrb ring, and the backend
/// drains it. What AAudio adds is what Android decides, not koan.
///
/// There is no device list to choose from and no nominal rate to set. What
/// stands in for a device is the route the system is playing to, which the
/// app owns (it hears route changes through `AudioManager`) and tells us of
/// through `set_route`. A stream is opened at the source's rate: exclusive
/// where the device allows it, which is the only case in which the hardware
/// runs at that rate, and shared otherwise, where the system mixer converts.
/// What was granted is read back from the stream, not assumed.
///
/// When the route changes the system disconnects the stream and says so in
/// its error callback. That callback may not close the stream or open another,
/// so a worker thread does: it reopens on whatever the system now routes to,
/// keeping the ring, the fader and the play position, and carries on if
/// playback was running.
pub struct AndroidAudioBackend;

/// The name of the route as the app last heard it: "Speaker", "Pixel Buds",
/// "USB-C Headset". What DSP profiles are keyed on.
static ROUTE: parking_lot::RwLock<Option<String>> = parking_lot::RwLock::new(None);

/// The `AudioDeviceInfo.getId()` of the route, which AAudio takes as a device
/// id; 0 for whatever the system's default is.
static ROUTE_DEVICE: AtomicI32 = AtomicI32::new(0);

/// The rate the platform says the route runs at, in Hz; 0 while unknown.
static ROUTE_RATE: AtomicU32 = AtomicU32::new(0);

/// What the stream now open was granted.
static GRANTED: Mutex<Option<Granted>> = Mutex::new(None);

type RateWatch = Arc<dyn Fn(f64) + Send + Sync>;

/// Told the rate the hardware runs at when a stream is opened or the route
/// moves. One at a time: the player's current one.
static WATCH: Mutex<Option<RateWatch>> = Mutex::new(None);

/// Told by the app on each route change, and once at start. A new route is a
/// new output to every device controlling this one, so they are told.
///
/// `device_id` is the route's `AudioDeviceInfo.getId()`, or 0 to follow the
/// system's default; `sample_rate` is what the platform reports the route
/// runs at, or 0 if it does not say.
pub fn set_route(name: String, device_id: i32, sample_rate: u32) {
    let changed = ROUTE.write().replace(name.clone()).as_ref() != Some(&name);
    ROUTE_DEVICE.store(device_id, Ordering::Release);
    ROUTE_RATE.store(sample_rate, Ordering::Release);
    if changed {
        crate::remote::outputs::refresh_devices();
    }
}

/// What the most recent stream was granted, or `None` before any has opened.
/// Kept after the stream closes: it is the last the route is known to run at.
pub fn granted() -> Option<Granted> {
    *GRANTED.lock()
}

/// The rate the hardware runs at under the stream now open, as far as the
/// platform says: see `android_policy::hardware_rate`.
pub fn hardware_rate() -> Option<u32> {
    granted().map(|g| policy::hardware_rate(g, ROUTE_RATE.load(Ordering::Acquire)))
}

/// The route changed: say what the hardware now runs at.
pub(crate) fn follow_route() {
    if let Some(rate) = hardware_rate() {
        tell_watch(rate as f64);
    }
}

fn tell_watch(rate: f64) {
    let watch = WATCH.lock().clone();
    if let Some(watch) = watch {
        watch(rate);
    }
}

fn current_route() -> DeviceInfo {
    DeviceInfo {
        name: ROUTE
            .read()
            .clone()
            .unwrap_or_else(|| "System Output".to_string()),
        sample_rates: policy::LISTED_RATES.to_vec(),
        platform_id: ROUTE_DEVICE.load(Ordering::Acquire).max(0) as u64,
        kind: Default::default(),
    }
}

impl AudioBackend for AndroidAudioBackend {
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        Ok(vec![current_route()])
    }

    fn default_device(&self) -> Result<DeviceInfo, BackendError> {
        Ok(current_route())
    }

    fn supported_sample_rates(&self, _device: &DeviceInfo) -> Result<Vec<f64>, BackendError> {
        Ok(current_route().sample_rates)
    }

    fn get_device_sample_rate(&self, _device: &DeviceInfo) -> Result<f64, BackendError> {
        // Zero while unknown: no switch from an unknown rate is one to wait
        // out.
        Ok(hardware_rate()
            .unwrap_or_else(|| ROUTE_RATE.load(Ordering::Acquire))
            .into())
    }

    fn set_device_sample_rate(&self, _device: &DeviceInfo, rate: f64) -> Result<f64, BackendError> {
        // The rate is set by opening the stream. What the route will make of
        // it is not known until then, so this answers with the best guess
        // and the stream's grant corrects it through the watch.
        Ok(policy::expected_rate(
            rate,
            ROUTE_RATE.load(Ordering::Acquire),
        ))
    }

    fn watch_device_sample_rate(
        &self,
        _device: &DeviceInfo,
        on_change: Box<dyn Fn(f64) + Send + Sync>,
    ) -> Option<Box<dyn SampleRateWatch>> {
        let watch: RateWatch = Arc::from(on_change);
        *WATCH.lock() = Some(watch.clone());
        Some(Box::new(Watching(watch)))
    }

    fn create_engine(
        &self,
        device: &DeviceInfo,
        sample_rate: f64,
        channels: u32,
        consumer: rtrb::Consumer<f32>,
        samples_played: Arc<AtomicU64>,
    ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
        let rate = sample_rate.round() as u32;
        let fade = FadeControl::for_output();
        let output = Output {
            consumer,
            fader: Fader::new(fade.clone(), sample_rate),
            samples_played,
            channels: channels as usize,
        };
        let (reopen_tx, reopen_rx) = crossbeam_channel::unbounded();
        let inner = Arc::new(Inner {
            output: Arc::new(OutputCell(UnsafeCell::new(output))),
            running: Arc::new(AtomicBool::new(false)),
            fade,
            rate,
            channels: channels as i32,
            device_id: device.platform_id as i32,
            stream: Mutex::new(None),
            closed: AtomicBool::new(false),
            reopen_tx,
        });
        let (stream, granted) = inner.open().map_err(BackendError::StreamCreation)?;
        inner.keep(stream, granted);

        let worker = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("koan-aaudio".into())
            .spawn(move || reopen_worker(worker, reopen_rx))
            .map_err(|e| BackendError::StreamCreation(e.to_string()))?;
        Ok(Box::new(AndroidEngineHandle { inner }))
    }
}

/// The player's subscription to the hardware rate. A newer one replaces it,
/// so it clears the slot on drop only if it is still the one there.
struct Watching(RateWatch);

impl SampleRateWatch for Watching {}

impl Drop for Watching {
    fn drop(&mut self) {
        let mut slot = WATCH.lock();
        if slot.as_ref().is_some_and(|w| Arc::ptr_eq(w, &self.0)) {
            *slot = None;
        }
    }
}

/// `Output`, handed from one stream's callback to the next across a reopen.
struct OutputCell(UnsafeCell<Output>);

// SAFETY: at most one stream's callback runs at a time, which is what makes
// the `&mut` in `render` unique. A stream is opened without being started;
// only the one in `Inner::stream` is ever started; and the one it replaces is
// closed first, which does not return until its callback thread has exited.
unsafe impl Sync for OutputCell {}

struct SendStream(AudioStream);

// SAFETY: an AAudio stream may be driven from any thread; the NDK documents
// its calls as thread-safe. It is not `Send` in the crate only because it
// holds a raw pointer.
unsafe impl Send for SendStream {}

struct Inner {
    output: Arc<OutputCell>,
    running: Arc<AtomicBool>,
    fade: Arc<FadeControl>,
    rate: u32,
    channels: i32,
    device_id: i32,
    /// The stream now open; `None` between a disconnect and its reopening, or
    /// after every attempt to reopen has failed.
    stream: Mutex<Option<SendStream>>,
    closed: AtomicBool,
    /// Wakes the reopen worker. The error callback sends; it may do nothing
    /// else with the stream.
    reopen_tx: Sender<()>,
}

fn fault_of(err: AudioError) -> StreamFault {
    match err {
        AudioError::Disconnected => StreamFault::Disconnected,
        _ => StreamFault::Other,
    }
}

impl Inner {
    /// Open a stream, trying each way `android_policy::open_attempts` gives
    /// and keeping the first the system grants at the rate asked for. The
    /// stream is not started.
    fn open(&self) -> Result<(AudioStream, Granted), String> {
        let mut last = String::from("no attempt made");
        for attempt in policy::open_attempts(self.rate) {
            match self.open_once(attempt) {
                Ok(opened) => return Ok(opened),
                Err(e) => {
                    log::info!(
                        "aaudio: {:?} at {}Hz not opened: {e}",
                        attempt.sharing,
                        attempt.sample_rate
                    );
                    last = e;
                }
            }
        }
        Err(last)
    }

    fn open_once(&self, attempt: Attempt) -> Result<(AudioStream, Granted), String> {
        let channels = self.channels;
        let samples_per_frame = channels as usize;
        let cell = self.output.clone();
        let running = self.running.clone();
        let tx = self.reopen_tx.clone();

        let stream = AudioStreamBuilder::new()
            .map_err(|e| e.to_string())?
            .direction(AudioDirection::Output)
            .format(AudioFormat::PCM_Float)
            .channel_count(channels)
            .sample_rate(attempt.sample_rate as i32)
            .sharing_mode(match attempt.sharing {
                Sharing::Exclusive => AudioSharingMode::Exclusive,
                Sharing::Shared => AudioSharingMode::Shared,
            })
            .performance_mode(AudioPerformanceMode::LowLatency)
            .usage(AudioUsage::Media)
            .content_type(AudioContentType::Music)
            .device_id(self.device_id)
            .data_callback(Box::new(move |_stream, data, frames| {
                // SAFETY: AAudio hands over `frames` frames of the channel
                // count and format asked for. Uniqueness of the `Output`
                // borrow: see `OutputCell`.
                let (output, data) = unsafe {
                    (
                        &mut *cell.0.get(),
                        std::slice::from_raw_parts_mut(
                            data as *mut f32,
                            frames.max(0) as usize * samples_per_frame,
                        ),
                    )
                };
                render(output, &running, data);
                AudioCallbackResult::Continue
            }))
            .error_callback(Box::new(move |_stream, err| {
                // Not here: closing, reopening, or anything slow. The worker
                // does it.
                if policy::reopens(fault_of(err)) {
                    let _ = tx.send(());
                }
            }))
            .open_stream()
            .map_err(|e| e.to_string())?;

        if !policy::keeps(
            attempt,
            stream.sample_rate(),
            channels,
            stream.channel_count(),
        ) || stream.format() != AudioFormat::PCM_Float
        {
            return Err(format!(
                "granted {}Hz, {} channels, {:?}",
                stream.sample_rate(),
                stream.channel_count(),
                stream.format()
            ));
        }

        let granted = Granted {
            sharing: match stream.sharing_mode() {
                AudioSharingMode::Exclusive => Sharing::Exclusive,
                _ => Sharing::Shared,
            },
            sample_rate: stream.sample_rate() as u32,
            device_id: stream.device_id(),
        };
        Ok((stream, granted))
    }

    /// Make `stream` the open one, closing any it replaces, and say what the
    /// hardware now runs at.
    fn keep(&self, stream: AudioStream, granted: Granted) {
        log::info!(
            "aaudio: {:?} stream at {}Hz on device {}",
            granted.sharing,
            granted.sample_rate,
            granted.device_id
        );
        let old = self.stream.lock().replace(SendStream(stream));
        close(old);
        *GRANTED.lock() = Some(granted);
        tell_watch(policy::hardware_rate(granted, ROUTE_RATE.load(Ordering::Acquire)) as f64);
    }

    /// Reopen after a disconnect, backing off until the system settles on a
    /// route. Starts the new stream if playback was running.
    fn reopen(&self) {
        for delay in REOPEN_DELAYS {
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            // Closed first: the callback of the stream it replaces must have
            // exited before the new one can start (see `OutputCell`).
            close(self.stream.lock().take());
            match self.open() {
                Ok((stream, granted)) => {
                    self.keep(stream, granted);
                    if self.running.load(Ordering::Acquire)
                        && let Err(e) = self.start_stream()
                    {
                        log::warn!("aaudio: reopened stream did not start: {e}");
                    }
                    return;
                }
                Err(e) => log::warn!("aaudio: reopen failed: {e}"),
            }
        }
        log::error!("aaudio: gave up reopening the output");
    }

    fn start_stream(&self) -> Result<(), BackendError> {
        let slot = self.stream.lock();
        let stream = slot
            .as_ref()
            .ok_or_else(|| BackendError::Platform("no output stream".into()))?;
        stream
            .0
            .request_start()
            .map_err(|e| BackendError::Platform(e.to_string()))
    }
}

/// Close a stream. The NDK crate unwraps the result in `Drop`, and a failure
/// to close a stream already disconnected must not take a thread with it.
fn close(stream: Option<SendStream>) {
    if let Some(stream) = stream
        && std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(stream))).is_err()
    {
        log::warn!("aaudio: closing a stream failed");
    }
}

fn reopen_worker(inner: Weak<Inner>, wake: Receiver<()>) {
    while wake.recv().is_ok() {
        // A burst of errors is one disconnect.
        while wake.try_recv().is_ok() {}
        let Some(inner) = inner.upgrade() else { return };
        if inner.closed.load(Ordering::Acquire) {
            return;
        }
        inner.reopen();
    }
}

/// Handle to an AAudio output.
struct AndroidEngineHandle {
    inner: Arc<Inner>,
}

impl AudioEngineHandle for AndroidEngineHandle {
    fn start(&self) -> Result<(), BackendError> {
        self.inner.running.store(true, Ordering::Release);
        if self.inner.stream.lock().is_none() {
            // Every try at reopening failed: try once more, now that the
            // player is asking for sound.
            let (stream, granted) = self.inner.open().map_err(BackendError::StreamCreation)?;
            self.inner.keep(stream, granted);
        }
        self.inner.start_stream()
    }

    fn stop(&self) -> Result<(), BackendError> {
        self.inner.running.store(false, Ordering::Release);
        let slot = self.inner.stream.lock();
        let Some(stream) = slot.as_ref() else {
            return Ok(());
        };
        match stream.0.request_pause() {
            Ok(()) => Ok(()),
            // Disconnected or being reopened: the callback already plays
            // silence now `running` is clear, and the reopened stream is not
            // started.
            Err(AudioError::Disconnected | AudioError::InvalidState) => Ok(()),
            Err(e) => Err(BackendError::Platform(e.to_string())),
        }
    }

    fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::Acquire)
    }

    fn fade_out(&self) {
        self.inner.fade.fade_out();
    }

    fn fade_out_quickly(&self) {
        self.inner.fade.fade_out_quickly();
    }

    fn period(&self) -> std::time::Duration {
        self.inner.fade.period()
    }

    fn fade_in_quickly(&self) -> Result<(), BackendError> {
        if self.is_running() {
            self.inner.fade.fade_in_quickly(false);
            return Ok(());
        }
        self.inner.fade.fade_in_quickly(true);
        self.start()
    }

    fn set_sleep_gain(&self, gain: f32, snap: bool) {
        self.inner.fade.set_sleep_gain(gain, snap);
    }

    fn fade_in(&self) -> Result<(), BackendError> {
        if self.is_running() {
            self.inner.fade.fade_in(false);
            return Ok(());
        }
        self.inner.fade.fade_in(true);
        self.start()
    }

    fn is_silent(&self) -> bool {
        self.inner.fade.is_silent()
    }
}

impl Drop for AndroidEngineHandle {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::Release);
        self.inner.running.store(false, Ordering::Release);
        // Wakes the worker, which sees `closed` and returns.
        let _ = self.inner.reopen_tx.send(());
        close(self.inner.stream.lock().take());
    }
}
