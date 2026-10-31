//! koan-core in a browser. A dedicated worker loads a file into a [`Loader`],
//! which probes it into a [`Session`]; the session's `run` is the decode
//! thread, writing into an `rtrb` ring that an `AudioWorklet` instance of
//! this same module reads through [`render`], over the same shared memory.
//!
//! The worklet's agent cannot block, and the allocator's lock blocks when
//! contended, so nothing on the worklet's side allocates or frees once its
//! instance is up: the worker owns both halves of the ring and the scratch
//! buffer the worklet reads from, and frees them only after the audio
//! context has closed.

#![cfg(target_arch = "wasm32")]

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use koan_core::audio::buffer::{self, PlaybackTimeline, Processing, SourceEntry, StreamInfo};
use koan_core::queue_id::QueueItemId;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use wasm_bindgen::prelude::*;

/// The most samples one `render` call hands over: well past a render
/// quantum (128 frames today) at any channel count the client accepts.
const SCRATCH_SAMPLES: usize = 8192;

/// Channels the client plays. The destination mixes only 1, 2, 4 and 6 to
/// the speakers; any other count is played discretely, dropping channels.
const MAX_CHANNELS: u16 = 2;

/// Route `log` to the console and panics to `console.error`. The worker calls
/// this once; the worklet does not, since both allocate.
#[wasm_bindgen]
pub fn worker_init() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
}

/// A file arriving in chunks, held whole in memory once it has all arrived.
#[wasm_bindgen]
pub struct Loader {
    bytes: Vec<u8>,
    expected: usize,
}

#[wasm_bindgen]
impl Loader {
    /// Room for a file of `length` bytes, its `Content-Length`. Refused rather
    /// than aborting the instance when memory has no room for it.
    #[wasm_bindgen(constructor)]
    pub fn new(length: f64) -> Result<Loader, JsError> {
        if !(length.is_finite() && length >= 1.0 && length <= usize::MAX as f64) {
            return Err(JsError::new(&format!("{length} is not a file length")));
        }
        let expected = length as usize;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(expected)
            .map_err(|_| JsError::new(&format!("no room in memory for {expected} bytes")))?;
        Ok(Self { bytes, expected })
    }

    /// The next chunk. A body longer than its length is refused: holding the
    /// extra would reallocate, doubling the memory the file takes.
    pub fn push(&mut self, chunk: &[u8]) -> Result<(), JsError> {
        if self.bytes.len() + chunk.len() > self.expected {
            return Err(JsError::new("the file is longer than the server said"));
        }
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }

    /// Probe the whole file and set up the ring it will be decoded into.
    pub fn finish(self) -> Result<Session, JsError> {
        if self.bytes.len() != self.expected {
            return Err(JsError::new(&format!(
                "the file ended after {} of {} bytes",
                self.bytes.len(),
                self.expected
            )));
        }
        let source = Shared(Arc::new(self.bytes));
        let info = buffer::probe_source(mss(source.clone()), &Hint::new())
            .map_err(|e| JsError::new(&format!("koan cannot play this file: {e}")))?;
        if info.channels == 0 || info.channels > MAX_CHANNELS {
            return Err(JsError::new(&format!(
                "the web client plays mono and stereo, not {} channels",
                info.channels
            )));
        }
        let (producer, consumer) = rtrb::RingBuffer::new(buffer::RING_BUFFER_SIZE);
        Ok(Session {
            info,
            source: Some(source),
            producer: Some(producer),
            consumer: Box::into_raw(Box::new(consumer)),
            scratch: Box::into_raw(vec![0.0; SCRATCH_SAMPLES].into_boxed_slice()),
            stop: Arc::new(AtomicBool::new(false)),
            timeline: PlaybackTimeline::new(),
        })
    }
}

/// One file, decoded once into the ring.
#[wasm_bindgen]
pub struct Session {
    info: StreamInfo,
    source: Option<Shared>,
    producer: Option<rtrb::Producer<f32>>,
    /// The ring's consumer and the buffer `render` fills, lent to the worklet
    /// as raw pointers and owned here: freed in `drop`, never through a
    /// reference the worklet's writes would alias.
    consumer: *mut rtrb::Consumer<f32>,
    scratch: *mut [f32],
    stop: Arc<AtomicBool>,
    timeline: Arc<PlaybackTimeline>,
}

#[wasm_bindgen]
impl Session {
    #[wasm_bindgen(getter)]
    pub fn sample_rate(&self) -> u32 {
        self.info.sample_rate
    }

    #[wasm_bindgen(getter)]
    pub fn channels(&self) -> u16 {
        self.info.channels
    }

    #[wasm_bindgen(getter)]
    pub fn codec(&self) -> String {
        self.info.codec.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn bit_depth(&self) -> Option<u16> {
        self.info.bit_depth
    }

    #[wasm_bindgen(getter)]
    pub fn duration_ms(&self) -> f64 {
        self.info.duration_ms as f64
    }

    /// The ring's consumer, for [`render`]. Valid until the session is freed.
    #[wasm_bindgen(getter)]
    pub fn consumer(&self) -> usize {
        self.consumer as usize
    }

    /// Where [`render`] writes, for the worklet to read from.
    #[wasm_bindgen(getter)]
    pub fn scratch(&self) -> usize {
        self.scratch as *mut f32 as usize
    }

    #[wasm_bindgen(getter)]
    pub fn scratch_len(&self) -> usize {
        SCRATCH_SAMPLES
    }

    /// The stop flag's byte: the page sets it with `Atomics.store`, since the
    /// worker hears no message while `run` holds it.
    #[wasm_bindgen(getter)]
    pub fn stop(&self) -> usize {
        self.stop.as_ptr() as usize
    }

    /// Decode the file into the ring on this thread, returning once its audio
    /// has drained or the stop flag is set. Blocks the worker throughout.
    pub fn run(&mut self) -> Result<(), JsError> {
        let (Some(source), Some(producer)) = (self.source.take(), self.producer.take()) else {
            return Err(JsError::new("this session has already been played"));
        };
        let entry = SourceEntry {
            id: QueueItemId::new(),
            path: "stream".into(),
            hint: Hint::new(),
            make_mss: Box::new(move || Ok(mss(source))),
        };
        buffer::run_decode(
            entry,
            producer,
            &self.stop,
            0,
            &|| None,
            &self.timeline,
            None,
            &Processing::default(),
        );
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: both came from `Box::into_raw` and are freed only here; the
        // page frees a session only after its audio context has closed, so
        // the worklet no longer reads them.
        unsafe {
            drop(Box::from_raw(self.consumer));
            drop(Box::from_raw(self.scratch));
        }
    }
}

/// Move up to `len` samples from the ring to `scratch`, whole frames of
/// `channels` only, and answer how many. The worklet's render callback: no
/// allocation, no locks, nothing that can panic.
///
/// `consumer` and `scratch` are a live session's `consumer` and `scratch`,
/// and `len` at most its `scratch_len`; the worklet is their only user while
/// it runs.
#[wasm_bindgen]
pub fn render(consumer: usize, scratch: usize, len: usize, channels: usize) -> usize {
    if consumer == 0 || scratch == 0 || channels == 0 {
        return 0;
    }
    // SAFETY: the caller passes a live session's pointers, which the worker
    // frees only after the audio context has closed, and nothing else reads
    // the consumer or touches the scratch buffer meanwhile.
    let (consumer, out) = unsafe {
        (
            &mut *(consumer as *mut rtrb::Consumer<f32>),
            std::slice::from_raw_parts_mut(scratch as *mut f32, len.min(SCRATCH_SAMPLES)),
        )
    };
    let n = consumer.slots().min(out.len()) / channels * channels;
    let Ok(chunk) = consumer.read_chunk(n) else {
        return 0;
    };
    let (first, second) = chunk.as_slices();
    let (head, tail) = out.split_at_mut(first.len());
    head.copy_from_slice(first);
    if let Some(tail) = tail.get_mut(..second.len()) {
        tail.copy_from_slice(second);
    }
    chunk.commit_all();
    n
}

/// The file's bytes, shared between the probe and the decoder without a copy.
#[derive(Clone)]
struct Shared(Arc<Vec<u8>>);

impl AsRef<[u8]> for Shared {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

fn mss(source: Shared) -> MediaSourceStream<'static> {
    MediaSourceStream::new(Box::new(Cursor::new(source)), Default::default())
}
